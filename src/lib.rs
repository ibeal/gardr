use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn specs_path(&self) -> PathBuf {
        self.root.join("specs")
    }
    pub fn runs_path(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub fn mounts_path(&self) -> PathBuf {
        self.root.join("mounts")
    }
    pub fn images_path(&self) -> PathBuf {
        self.root.join("images")
    }
    pub fn spec_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("spec name", name)?;
        Ok(self.specs_path().join(format!("{name}.toml")))
    }
    pub fn run_path(&self, id: &str) -> Result<PathBuf> {
        validate_name("run id", id)?;
        Ok(self.runs_path().join(id))
    }

    pub fn add_spec(&self, name: &str, source: &Path) -> Result<SpecIdentity> {
        let destination = self.spec_path(name)?;
        if destination.exists() {
            return Err(format!("spec already exists: {name}"));
        }
        let content = fs::read(source).map_err(io_error)?;
        let spec = parse_spec(&content)?;
        validate_spec(&spec)?;
        fs::create_dir_all(self.specs_path()).map_err(io_error)?;
        write_new(&destination, &content)?;
        Ok(SpecIdentity {
            name: name.to_owned(),
            sha256: digest(&content),
        })
    }

    pub fn read_spec(&self, name: &str) -> Result<(Spec, SpecIdentity, Vec<u8>)> {
        let path = self.spec_path(name)?;
        let content = fs::read(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("spec does not exist: {name}")
            } else {
                io_error(error)
            }
        })?;
        let spec = parse_spec(&content)?;
        validate_spec(&spec)?;
        Ok((
            spec,
            SpecIdentity {
                name: name.to_owned(),
                sha256: digest(&content),
            },
            content,
        ))
    }

    pub fn list_specs(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !self.specs_path().exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(self.specs_path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            if entry.file_type().map_err(io_error)?.is_file()
                && path.extension().is_some_and(|x| x == "toml")
            {
                names.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn create_run(&self, workspace: &Path, spec_name: &str) -> Result<(RunRecord, Spec)> {
        let workspace = validate_workspace(workspace)?;
        let (spec, identity, content) = self.read_spec(spec_name)?;
        validate_runtime_spec(self, &spec)?;
        fs::create_dir_all(self.runs_path()).map_err(io_error)?;
        let id = new_run_id();
        let directory = self.run_path(&id)?;
        fs::create_dir(&directory).map_err(io_error)?;
        let record = RunRecord {
            version: 1,
            id: id.clone(),
            state: RunState::Prepared,
            spec: identity,
            workspace: workspace.display().to_string(),
            image: None,
            container: None,
            created_at: now(),
            updated_at: now(),
            exit_status: None,
            failure: None,
            state_path: directory.display().to_string(),
            log_path: directory.join("runner.log").display().to_string(),
            artifact_path: directory.join("artifacts").display().to_string(),
        };
        write_new(&directory.join("spec.toml"), &content)?;
        write_new(
            &directory.join("workspace-seal.sha256"),
            workspace_seal_digest(&workspace)?.as_bytes(),
        )?;
        let mounts = lock_mounts(self, &spec)?;
        write_new(
            &directory.join("mounts.json"),
            &serde_json::to_vec_pretty(&mounts).map_err(|error| error.to_string())?,
        )?;
        fs::create_dir(directory.join("artifacts")).map_err(io_error)?;
        save_record(&directory, &record)?;
        Ok((record, spec))
    }

    pub fn read_run(&self, id: &str) -> Result<RunRecord> {
        let path = self.run_path(id)?.join("state.json");
        let bytes = fs::read(path).map_err(io_error)?;
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid run state: {error}"))
    }

    pub fn resume(&self, id: &str) -> Result<RunRecord> {
        let mut record = self.read_run(id)?;
        if matches!(record.state, RunState::Running) {
            let container = record
                .container
                .as_deref()
                .ok_or_else(|| "running run has no container identifier".to_owned())?;
            match docker_status(container)? {
                Some((false, exit_status)) => {
                    record.state = RunState::Stopped;
                    record.exit_status = exit_status;
                    record.updated_at = now();
                    save_record(&self.run_path(id)?, &record)?;
                }
                Some((true, _)) => {}
                None => return Err("container is no longer available".to_owned()),
            }
        }
        if !matches!(
            record.state,
            RunState::Prepared | RunState::Stopped | RunState::Failed
        ) {
            return Err(format!(
                "run {id} is not resumable from state {}",
                record.state
            ));
        }
        let directory = self.run_path(id)?;
        if directory.join("cleanup.complete").exists() {
            return Err("cleaned runs are terminal and cannot be resumed".to_owned());
        }
        let workspace = validate_workspace(Path::new(&record.workspace))?;
        let recorded_seal =
            fs::read_to_string(directory.join("workspace-seal.sha256")).map_err(io_error)?;
        if recorded_seal != workspace_seal_digest(&workspace)? {
            return Err(
                "workspace sealed dispatch inventory changed since the run was created".to_owned(),
            );
        }
        let frozen = fs::read(directory.join("spec.toml")).map_err(io_error)?;
        if digest(&frozen) != record.spec.sha256 {
            return Err("frozen run spec does not match recorded spec identity".to_owned());
        }
        let spec = parse_spec(&frozen)?;
        validate_spec(&spec)?;
        validate_runtime_spec(self, &spec)?;
        verify_locked_mounts(self, &spec, &directory)?;
        if !directory.join("resolved.json").is_file() || record.image.is_none() {
            return Err(
                "run has incomplete resolved configuration and cannot be resumed".to_owned(),
            );
        }
        self.launch(record, workspace, spec)
    }

    pub fn start(&self, workspace: &Path, spec_name: &str) -> Result<RunRecord> {
        let (record, spec) = self.create_run(workspace, spec_name)?;
        let workspace = PathBuf::from(&record.workspace);
        self.launch(record, workspace, spec)
    }

    pub fn observe(&self, id: &str) -> Result<RunRecord> {
        let mut record = self.read_run(id)?;
        if matches!(record.state, RunState::Running) {
            let container = record
                .container
                .as_deref()
                .ok_or_else(|| "running run has no container identifier".to_owned())?;
            match docker_status(container)? {
                Some((true, _)) => {}
                Some((false, exit_status)) => {
                    record.state = RunState::Stopped;
                    record.exit_status = exit_status;
                }
                None => {
                    record.state = RunState::Failed;
                    record.failure = Some("container is no longer available".to_owned());
                }
            }
        }
        Ok(record)
    }

    pub fn stop(&self, id: &str) -> Result<RunRecord> {
        let mut record = self.read_run(id)?;
        if !matches!(record.state, RunState::Running) {
            return Err(format!("run {id} is not running"));
        }
        let container = record
            .container
            .as_deref()
            .ok_or_else(|| "running run has no container identifier".to_owned())?;
        match docker_status(container)? {
            Some((true, _)) => {}
            Some((false, exit_status)) => {
                record.state = RunState::Stopped;
                record.exit_status = exit_status;
                record.updated_at = now();
                save_record(&self.run_path(id)?, &record)?;
                return Ok(record);
            }
            None => return Err("container is no longer available".to_owned()),
        }
        let output = Command::new("docker")
            .args(["stop", container])
            .output()
            .map_err(io_error)?;
        if !output.status.success() {
            return Err(command_error("docker stop", &output));
        }
        record.state = RunState::Stopped;
        record.updated_at = now();
        save_record(&self.run_path(id)?, &record)?;
        Ok(record)
    }

    pub fn cleanup(&self, id: &str) -> Result<()> {
        let record = self.observe(id)?;
        if matches!(record.state, RunState::Running) {
            return Err("refusing to clean up a running run; stop it first".to_owned());
        }
        let directory = self.run_path(id)?;
        if directory.join("cleanup.complete").exists() {
            return Ok(());
        }
        if let Some(container) = record.container {
            let output = Command::new("docker")
                .args(["rm", &container])
                .output()
                .map_err(io_error)?;
            if !output.status.success()
                && !String::from_utf8_lossy(&output.stderr).contains("No such container")
            {
                return Err(command_error("docker rm", &output));
            }
        }
        write_new(&directory.join("cleanup.complete"), b"cleaned\n")
    }

    fn launch(&self, mut record: RunRecord, workspace: PathBuf, spec: Spec) -> Result<RunRecord> {
        let directory = self.run_path(&record.id)?;
        let image = match &record.image {
            Some(image) => image.clone(),
            None => match ensure_image(self, &spec) {
                Ok(image) => {
                    record.image = Some(image.clone());
                    let resolved =
                        serde_json::to_vec_pretty(&ResolvedConfig::from_spec(&record, &spec, self))
                            .map_err(|error| error.to_string())?;
                    write_new(&directory.join("resolved.json"), &resolved)?;
                    save_record(&directory, &record)?;
                    image
                }
                Err(error) => return fail_run(&directory, &mut record, error),
            },
        };
        let arguments = match docker_arguments(self, &spec, &workspace, &image) {
            Ok(arguments) => arguments,
            Err(error) => return fail_run(&directory, &mut record, error),
        };
        let output = match Command::new("docker").args(&arguments).output() {
            Ok(output) => output,
            Err(error) => return fail_run(&directory, &mut record, io_error(error)),
        };
        if !output.status.success() {
            return fail_run(
                &directory,
                &mut record,
                command_error("docker run", &output),
            );
        }
        let container = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if container.is_empty() {
            return fail_run(
                &directory,
                &mut record,
                "docker run did not return a container identifier".to_owned(),
            );
        }
        record.state = RunState::Running;
        record.container = Some(container);
        record.updated_at = now();
        append_log(&directory, "container launched")?;
        save_record(&directory, &record)?;
        Ok(record)
    }
}

fn fail_run(directory: &Path, record: &mut RunRecord, error: String) -> Result<RunRecord> {
    record.state = RunState::Failed;
    record.failure = Some(error.clone());
    record.updated_at = now();
    append_log(directory, &error)?;
    save_record(directory, record)?;
    Err(error)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub version: u32,
    pub image: Image,
    pub sandbox: Sandbox,
    pub harness: Harness,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub credentials: Credentials,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub reference: String,
    #[serde(default)]
    pub build_context: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    pub network: Network,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    None,
    Bridge,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    pub adapter: Adapter,
    #[serde(default)]
    pub command: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
    ClaudeCode,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    #[serde(default)]
    pub environment: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpecIdentity {
    pub name: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub version: u32,
    pub id: String,
    pub state: RunState,
    pub spec: SpecIdentity,
    pub workspace: String,
    pub image: Option<String>,
    pub container: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub exit_status: Option<i32>,
    pub failure: Option<String>,
    pub state_path: String,
    pub log_path: String,
    pub artifact_path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Prepared,
    Running,
    Stopped,
    Failed,
}
impl std::fmt::Display for RunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap().trim_matches('"')
        )
    }
}

#[derive(Serialize)]
struct ResolvedConfig<'a> {
    run_id: &'a str,
    workspace: String,
    spec: &'a SpecIdentity,
    image: &'a Image,
    image_id: &'a Option<String>,
    sandbox: &'a Sandbox,
    harness: &'a Harness,
    mounts: Vec<ResolvedMount>,
    credentials: &'a Credentials,
}
#[derive(Serialize)]
struct ResolvedMount {
    name: String,
    source: String,
    target: String,
    read_only: bool,
}
#[derive(Serialize, Deserialize)]
struct MountLock {
    name: String,
    source: String,
    sha256: String,
}
impl<'a> ResolvedConfig<'a> {
    fn from_spec(record: &'a RunRecord, spec: &'a Spec, store: &Store) -> Self {
        Self {
            run_id: &record.id,
            workspace: record.workspace.clone(),
            spec: &record.spec,
            image: &spec.image,
            image_id: &record.image,
            sandbox: &spec.sandbox,
            harness: &spec.harness,
            mounts: spec
                .mounts
                .iter()
                .map(|mount| ResolvedMount {
                    name: mount.name.clone(),
                    source: store.mounts_path().join(&mount.name).display().to_string(),
                    target: mount.target.clone(),
                    read_only: mount.read_only,
                })
                .collect(),
            credentials: &spec.credentials,
        }
    }
}

pub fn parse_spec(content: &[u8]) -> Result<Spec> {
    toml::from_str(
        std::str::from_utf8(content).map_err(|error| format!("run spec is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("invalid run spec: {error}"))
}
pub fn validate_spec(spec: &Spec) -> Result<()> {
    if spec.version != 1 {
        return Err("run spec version must be 1".to_owned());
    }
    if spec.image.reference.trim().is_empty() {
        return Err("image.reference is required".to_owned());
    }
    if let Some(context) = &spec.image.build_context {
        validate_name("image build_context", context)?;
    }
    if spec.harness.command.is_empty() {
        return Err("harness.command is required".to_owned());
    }
    if !matches!(spec.harness.adapter, Adapter::ClaudeCode)
        || spec
            .harness
            .command
            .first()
            .is_none_or(|command| command != "claude")
    {
        return Err(
            "the claude-code adapter requires a command beginning with `claude`".to_owned(),
        );
    }
    let mut names = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for mount in &spec.mounts {
        validate_name("mount name", &mount.name)?;
        if !names.insert(&mount.name) {
            return Err(format!("duplicate mount name: {}", mount.name));
        }
        validate_container_path("mount target", &mount.target)?;
        if mount.target == "/workspace" {
            return Err("/workspace is reserved for the prepared workspace".to_owned());
        }
        if !targets.insert(&mount.target) {
            return Err(format!("duplicate mount target: {}", mount.target));
        }
    }
    for reference in &spec.credentials.environment {
        validate_environment_reference(reference)?;
    }
    Ok(())
}

fn validate_runtime_spec(store: &Store, spec: &Spec) -> Result<()> {
    for mount in &spec.mounts {
        approved_child(&store.mounts_path(), &mount.name)?;
    }
    if let Some(context) = &spec.image.build_context
        && !approved_child(&store.images_path(), context)?
            .join("Dockerfile")
            .is_file()
    {
        return Err(format!(
            "image build context is missing Dockerfile: {context}"
        ));
    }
    Ok(())
}
fn lock_mounts(store: &Store, spec: &Spec) -> Result<Vec<MountLock>> {
    spec.mounts
        .iter()
        .map(|mount| {
            let source = approved_child(&store.mounts_path(), &mount.name)?;
            Ok(MountLock {
                name: mount.name.clone(),
                source: source.display().to_string(),
                sha256: digest_directory(&source)?,
            })
        })
        .collect()
}
fn verify_locked_mounts(store: &Store, spec: &Spec, directory: &Path) -> Result<()> {
    let locked: Vec<MountLock> =
        serde_json::from_slice(&fs::read(directory.join("mounts.json")).map_err(io_error)?)
            .map_err(|error| format!("invalid resolved mount lock: {error}"))?;
    let current = lock_mounts(store, spec)?;
    if locked.len() != current.len()
        || locked.iter().zip(current.iter()).any(|(left, right)| {
            left.name != right.name || left.source != right.source || left.sha256 != right.sha256
        })
    {
        return Err("approved mounts changed since the run was created".to_owned());
    }
    Ok(())
}

pub fn validate_workspace(path: &Path) -> Result<PathBuf> {
    let workspace = path.canonicalize().map_err(io_error)?;
    if !workspace.is_dir() {
        return Err("workspace must be a directory".to_owned());
    }
    let dispatches = workspace.join("dispatches");
    if !dispatches.is_dir() {
        return Err("workspace is missing dispatches/".to_owned());
    }
    let mut sealed = false;
    for entry in fs::read_dir(&dispatches).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_dir() {
            return Err("workspace dispatches contains a non-directory entry".to_owned());
        }
        let dispatch = entry.path();
        let record = dispatch.join("dispatch.json");
        if !record.is_file() {
            return Err(format!(
                "workspace dispatch is not sealed: {}",
                dispatch.display()
            ));
        }
        sealed = true;
        verify_dispatch(&dispatch, &record)?;
    }
    if !sealed {
        return Err("workspace has no sealed dispatch inventory".to_owned());
    }
    Ok(workspace)
}
fn workspace_seal_digest(workspace: &Path) -> Result<String> {
    let mut seals = Vec::new();
    for entry in fs::read_dir(workspace.join("dispatches")).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if entry.file_type().map_err(io_error)?.is_dir() {
            let path = entry.path().join("dispatch.json");
            seals.push((
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(path).map_err(io_error)?,
            ));
        }
    }
    seals.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (name, content) in seals {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(content);
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_dispatch(directory: &Path, record: &Path) -> Result<()> {
    #[derive(Deserialize)]
    struct Inventory {
        version: u32,
        files: Vec<InventoryEntry>,
    }
    #[derive(Deserialize)]
    struct InventoryEntry {
        path: String,
        sha256: String,
    }
    let inventory: Inventory = serde_json::from_slice(&fs::read(record).map_err(io_error)?)
        .map_err(|error| format!("invalid sealed dispatch inventory: {error}"))?;
    if inventory.version != 1 {
        return Err("unsupported sealed dispatch inventory version".to_owned());
    }
    let mut expected = BTreeSet::new();
    for item in inventory.files {
        let relative = validate_relative_path(Path::new(&item.path))?;
        if !expected.insert((relative, item.sha256)) {
            return Err("sealed dispatch contains duplicate inventory paths".to_owned());
        }
    }
    let actual = inventory_files(directory)?;
    if expected != actual {
        return Err(format!(
            "sealed dispatch inventory does not match: {}",
            directory.display()
        ));
    }
    if !directory.join("HANDOFF.json").is_file() {
        return Err("sealed dispatch is missing HANDOFF.json".to_owned());
    }
    Ok(())
}

fn inventory_files(base: &Path) -> Result<BTreeSet<(PathBuf, String)>> {
    let mut result = BTreeSet::new();
    collect_inventory(base, base, &mut result)?;
    Ok(result)
}
fn collect_inventory(
    base: &Path,
    directory: &Path,
    result: &mut BTreeSet<(PathBuf, String)>,
) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            collect_inventory(base, &path, result)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(base)
                .map_err(|error| error.to_string())?
                .to_path_buf();
            if relative != Path::new("dispatch.json") && relative != Path::new("HANDOFF.json") {
                result.insert((relative, digest(&fs::read(path).map_err(io_error)?)));
            }
        } else {
            return Err(format!(
                "sealed dispatch contains unsupported entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn ensure_image(store: &Store, spec: &Spec) -> Result<String> {
    if let Some(context) = &spec.image.build_context {
        let context_path = approved_child(&store.images_path(), context)?;
        let tag = format!("gardr-{}", digest_directory(&context_path)?);
        let output = Command::new("docker")
            .args(["image", "inspect", &tag])
            .output()
            .map_err(io_error)?;
        if !output.status.success() {
            let build = Command::new("docker")
                .arg("build")
                .args(["--tag", &tag])
                .arg(context_path)
                .output()
                .map_err(io_error)?;
            if !build.status.success() {
                return Err(command_error("docker build", &build));
            }
        }
        image_id(&tag)
    } else {
        let inspect = Command::new("docker")
            .args(["image", "inspect", &spec.image.reference])
            .output()
            .map_err(io_error)?;
        if !inspect.status.success() {
            let pull = Command::new("docker")
                .args(["pull", &spec.image.reference])
                .output()
                .map_err(io_error)?;
            if !pull.status.success() {
                return Err(command_error("docker pull", &pull));
            }
        }
        image_id(&spec.image.reference)
    }
}

fn docker_arguments(
    store: &Store,
    spec: &Spec,
    workspace: &Path,
    image: &str,
) -> Result<Vec<String>> {
    validate_docker_path("workspace path", workspace)?;
    let mut args = vec![
        "run".to_owned(),
        "--detach".to_owned(),
        "--network".to_owned(),
        match spec.sandbox.network {
            Network::None => "none",
            Network::Bridge => "bridge",
        }
        .to_owned(),
        "--mount".to_owned(),
        format!("type=bind,source={},target=/workspace", workspace.display()),
    ];
    for mount in &spec.mounts {
        let source = approved_child(&store.mounts_path(), &mount.name)?;
        let readonly = if mount.read_only { ",readonly" } else { "" };
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target={}{}",
                source.display(),
                mount.target,
                readonly
            ),
        ]);
    }
    for reference in &spec.credentials.environment {
        if std::env::var_os(reference).is_none() {
            return Err(format!("required credential is not set: {reference}"));
        }
        args.extend(["--env".to_owned(), reference.clone()]);
    }
    args.push(image.to_owned());
    args.extend(spec.harness.command.clone());
    Ok(args)
}

fn save_record(directory: &Path, record: &RunRecord) -> Result<()> {
    atomic_write(
        &directory.join("state.json"),
        &serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?,
    )
}
fn append_log(directory: &Path, message: &str) -> Result<()> {
    use std::io::Write;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("runner.log"))
        .map_err(io_error)?;
    writeln!(file, "{} {message}", now()).map_err(io_error)
}
fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    use std::io::Write;
    file.write_all(contents).map_err(io_error)
}
fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let temporary = path.with_extension("new");
    fs::write(&temporary, contents).map_err(io_error)?;
    fs::rename(temporary, path).map_err(io_error)
}
fn approved_child(root: &Path, name: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("approved store root is unavailable: {error}"))?;
    let candidate = root.join(name);
    if fs::symlink_metadata(&candidate)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "approved store entry must not be a symlink: {name}"
        ));
    }
    let resolved = candidate.canonicalize().map_err(io_error)?;
    if !resolved.starts_with(&root) || !resolved.is_dir() {
        return Err(format!("approved store entry escapes its root: {name}"));
    }
    Ok(resolved)
}
fn digest_directory(directory: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_digest_files(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (path, contents) in files {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(contents);
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn collect_digest_files(
    base: &Path,
    directory: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            return Err(format!(
                "image build context contains a symlink: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            collect_digest_files(base, &path, files)?;
        } else if kind.is_file() {
            files.push((
                path.strip_prefix(base)
                    .map_err(|error| error.to_string())?
                    .display()
                    .to_string(),
                fs::read(path).map_err(io_error)?,
            ));
        } else {
            return Err(format!(
                "image build context contains unsupported entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}
fn image_id(reference: &str) -> Result<String> {
    let output = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", reference])
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(command_error("docker image inspect", &output));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if id.is_empty() {
        return Err("docker image inspect did not return an immutable image ID".to_owned());
    }
    Ok(id)
}
fn docker_status(container: &str) -> Result<Option<(bool, Option<i32>)>> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Running}} {{.State.ExitCode}}",
            container,
        ])
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("No such") {
            return Ok(None);
        }
        return Err(command_error("docker inspect", &output));
    }
    let fields = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if fields.len() != 2 {
        return Err("docker inspect returned an invalid lifecycle response".to_owned());
    }
    let running = match fields[0].as_str() {
        "true" => true,
        "false" => false,
        _ => return Err("docker inspect returned an invalid running state".to_owned()),
    };
    let exit = fields[1]
        .parse::<i32>()
        .map_err(|_| "docker inspect returned an invalid exit status".to_owned())?;
    Ok(Some((running, (!running).then_some(exit))))
}
fn digest(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn new_run_id() -> String {
    format!(
        "run-{}-{}-{}",
        now(),
        std::process::id(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn command_error(name: &str, output: &std::process::Output) -> String {
    format!(
        "{name} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}
fn io_error(error: io::Error) -> String {
    error.to_string()
}
fn validate_name(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}
fn validate_environment_reference(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
    {
        Err(format!("invalid credential environment reference: {value}"))
    } else {
        Ok(())
    }
}
fn validate_container_path(label: &str, value: &str) -> Result<()> {
    if !value.starts_with('/')
        || value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value.contains('\0')
        || value.contains(',')
        || value.contains('\n')
        || value.contains('\r')
    {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}
fn validate_docker_path(label: &str, value: &Path) -> Result<()> {
    let value = value.to_string_lossy();
    if value.contains(',') || value.contains('\n') || value.contains('\r') || value.contains('\0') {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}
fn validate_relative_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        Err(format!("invalid relative path: {}", path.display()))
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gardr-test-{}-{}",
            now(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn spec() -> &'static [u8] {
        b"version = 1\n[image]\nreference = 'example:latest'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'claude-code'\ncommand = ['claude', '-p']\n"
    }
    #[test]
    fn specs_are_validated_and_immutable() {
        let temp = temporary_directory();
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let store = Store::open(temp.join("store"));
        let identity = store.add_spec("build", &source).unwrap();
        assert_eq!(identity.name, "build");
        assert_eq!(store.list_specs().unwrap(), ["build"]);
        assert!(store.add_spec("build", &source).is_err());
        assert!(store.read_spec("build").is_ok());
        fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn workspace_validation_detects_changed_sealed_input() {
        let temp = temporary_directory();
        let workspace = temp.join("workspace");
        let dispatch = workspace.join("dispatches/build");
        fs::create_dir_all(&dispatch).unwrap();
        fs::write(dispatch.join("AGENTS.md"), b"instructions").unwrap();
        fs::write(dispatch.join("HANDOFF.json"), b"{}\n").unwrap();
        let hash = digest(b"instructions");
        fs::write(
            dispatch.join("dispatch.json"),
            format!(
                "{{\"version\":1,\"files\":[{{\"path\":\"AGENTS.md\",\"sha256\":\"{hash}\"}}]}}"
            ),
        )
        .unwrap();
        validate_workspace(&workspace).unwrap();
        fs::write(dispatch.join("AGENTS.md"), b"changed").unwrap();
        assert!(
            validate_workspace(&workspace)
                .unwrap_err()
                .contains("does not match")
        );
        fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn rejects_unapproved_spec_fields() {
        assert!(parse_spec(b"version=1\nextra=true\n[image]\nreference='x'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='claude-code'\ncommand=['x']\n").is_err());
    }

    #[test]
    fn rejects_workspace_mount_aliases_and_unsealed_dispatches() {
        let spec = parse_spec(b"version=1\n[image]\nreference='x'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='claude-code'\ncommand=['claude']\n[[mounts]]\nname='tools'\ntarget='/workspace/.'\n").unwrap();
        assert!(validate_spec(&spec).is_err());

        let temp = temporary_directory();
        let workspace = temp.join("workspace");
        fs::create_dir_all(workspace.join("dispatches/unsealed")).unwrap();
        assert!(
            validate_workspace(&workspace)
                .unwrap_err()
                .contains("not sealed")
        );
        fs::remove_dir_all(temp).unwrap();
    }
}

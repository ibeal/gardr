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
    pub fn image_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("image profile name", name)?;
        Ok(self.images_path().join(format!("{name}.toml")))
    }
    pub fn pi_agent_path(&self) -> PathBuf {
        self.root.join("pi").join("agent")
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
        self.validate_image_requirements(&spec)?;
        fs::create_dir_all(self.specs_path()).map_err(io_error)?;
        write_new(&destination, &content)?;
        Ok(SpecIdentity {
            name: name.to_owned(),
            sha256: digest(&content),
        })
    }

    pub fn add_image(&self, name: &str, source: &Path) -> Result<SpecIdentity> {
        let destination = self.image_path(name)?;
        if destination.exists() {
            return Err(format!("image profile already exists: {name}"));
        }
        let content = fs::read(source).map_err(io_error)?;
        let image = parse_image_profile(&content)?;
        validate_image_profile(&image)?;
        validate_image_profile_runtime(self, &image)?;
        fs::create_dir_all(self.images_path()).map_err(io_error)?;
        write_new(&destination, &content)?;
        Ok(SpecIdentity {
            name: name.to_owned(),
            sha256: digest(&content),
        })
    }

    pub fn read_image(&self, name: &str) -> Result<(ImageProfile, SpecIdentity, Vec<u8>)> {
        let path = self.image_path(name)?;
        let content = fs::read(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("image profile does not exist: {name}")
            } else {
                io_error(error)
            }
        })?;
        let image = parse_image_profile(&content)?;
        validate_image_profile(&image)?;
        Ok((
            image,
            SpecIdentity {
                name: name.to_owned(),
                sha256: digest(&content),
            },
            content,
        ))
    }

    pub fn list_images(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !self.images_path().exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(self.images_path()).map_err(io_error)? {
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

    pub fn validate_image_requirements(&self, spec: &Spec) -> Result<SpecIdentity> {
        validate_image_requirements(self, spec)
    }

    pub fn validate_runtime_spec(&self, spec: &Spec) -> Result<SpecIdentity> {
        validate_runtime_spec(self, spec)
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

    pub fn create_run(
        &self,
        workspace: &Path,
        spec_name: &str,
        harness_args: Vec<String>,
    ) -> Result<(RunRecord, Spec)> {
        let workspace = validate_workspace(workspace)?;
        validate_harness_args(&harness_args)?;
        let (spec, identity, content) = self.read_spec(spec_name)?;
        validate_dispatch_harness_args(&spec, &harness_args)?;
        sync_pi_auth(self, &spec)?;
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
            image_profile: None,
            container: None,
            created_at: now(),
            updated_at: now(),
            exit_status: None,
            failure: None,
            state_path: directory.display().to_string(),
            log_path: directory.join("runner.log").display().to_string(),
            artifact_path: directory.join("artifacts").display().to_string(),
            harness_args,
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
        if !directory.join("resolved.json").is_file()
            || record.image.is_none()
            || record.image_profile.is_none()
        {
            return Err(
                "run has incomplete resolved configuration and cannot be resumed".to_owned(),
            );
        }
        self.launch(record, workspace, spec)
    }

    pub fn start(
        &self,
        workspace: &Path,
        spec_name: &str,
        harness_args: Vec<String>,
    ) -> Result<RunRecord> {
        let (record, spec) = self.create_run(workspace, spec_name, harness_args)?;
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
                Ok((image, profile)) => {
                    record.image = Some(image.clone());
                    record.image_profile = Some(profile);
                    let resolved =
                        serde_json::to_vec_pretty(&ResolvedConfig::from_spec(&record, &spec, self))
                            .map_err(|error| error.to_string())?;
                    write_new(&directory.join("resolved.json"), &resolved)?;
                    write_bootstrap(&directory, &spec)?;
                    save_record(&directory, &record)?;
                    image
                }
                Err(error) => return fail_run(&directory, &mut record, error),
            },
        };
        let arguments = match docker_arguments(
            self,
            &spec,
            &workspace,
            &directory,
            &record.spec.name,
            &record.id,
            &image,
            &record.harness_args,
        ) {
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
    #[serde(default)]
    pub firewall: Firewall,
    #[serde(default)]
    pub tools: Tooling,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub name: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageProfile {
    pub version: u32,
    pub source: ImageSource,
    #[serde(default)]
    pub harnesses: Vec<Adapter>,
    #[serde(default)]
    pub tools: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSource {
    #[serde(default)]
    pub reference: Option<String>,
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
    #[serde(default)]
    pub model: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
    ClaudeCode,
    Pi,
}
pub const CLAUDE_CODE_UNSUPPORTED: &str = "the claude-code adapter is not supported: it has no credential bootstrap yet; use the pi adapter with an Anthropic model instead";
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Firewall {
    #[serde(default)]
    pub allow: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tooling {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub install: Vec<Tool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub check: String,
    pub install: Vec<Vec<String>>,
    #[serde(default)]
    pub allow: Vec<String>,
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
    #[serde(default)]
    pub image_profile: Option<SpecIdentity>,
    pub container: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub exit_status: Option<i32>,
    pub failure: Option<String>,
    pub state_path: String,
    pub log_path: String,
    pub artifact_path: String,
    #[serde(default)]
    pub harness_args: Vec<String>,
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
    image_profile: &'a Option<SpecIdentity>,
    image_id: &'a Option<String>,
    sandbox: &'a Sandbox,
    harness: &'a Harness,
    mounts: Vec<ResolvedMount>,
    credentials: &'a Credentials,
    firewall: ResolvedFirewall,
    tools: &'a Tooling,
    pi_agent: Option<ResolvedPiAgent>,
}
#[derive(Serialize)]
struct ResolvedPiAgent {
    source: String,
    target: &'static str,
}
#[derive(Serialize)]
struct ResolvedFirewall {
    runtime: Vec<String>,
    effective: Vec<String>,
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
            image_profile: &record.image_profile,
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
            firewall: ResolvedFirewall {
                runtime: runtime_domains(spec),
                effective: installer_domains(spec),
            },
            tools: &spec.tools,
            pi_agent: matches!(spec.harness.adapter, Adapter::Pi).then(|| ResolvedPiAgent {
                source: store.pi_agent_path().display().to_string(),
                target: "/pi-agent",
            }),
        }
    }
}

pub fn parse_spec(content: &[u8]) -> Result<Spec> {
    toml::from_str(
        std::str::from_utf8(content).map_err(|error| format!("run spec is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("invalid run spec: {error}"))
}
pub fn parse_image_profile(content: &[u8]) -> Result<ImageProfile> {
    toml::from_str(
        std::str::from_utf8(content)
            .map_err(|error| format!("image profile is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("invalid image profile: {error}"))
}
pub fn validate_image_profile(image: &ImageProfile) -> Result<()> {
    if image.version != 1 {
        return Err("image profile version must be 1".to_owned());
    }
    if image
        .harnesses
        .iter()
        .any(|adapter| matches!(adapter, Adapter::ClaudeCode))
    {
        return Err(CLAUDE_CODE_UNSUPPORTED.to_owned());
    }
    match (&image.source.reference, &image.source.build_context) {
        (Some(reference), None) if !reference.trim().is_empty() => {}
        (None, Some(context)) => validate_name("image build_context", context)?,
        _ => {
            return Err(
                "image profile source requires exactly one of reference or build_context"
                    .to_owned(),
            );
        }
    }
    let mut tools = BTreeSet::new();
    for tool in &image.tools {
        validate_name("image profile tool", tool)?;
        if !tools.insert(tool) {
            return Err(format!("duplicate image profile tool: {tool}"));
        }
    }
    if image.harnesses.is_empty() {
        return Err("image profile harnesses are required".to_owned());
    }
    Ok(())
}
pub fn validate_spec(spec: &Spec) -> Result<()> {
    if spec.version != 1 {
        return Err("run spec version must be 1".to_owned());
    }
    validate_name("image name", &spec.image.name)?;
    if matches!(spec.harness.adapter, Adapter::ClaudeCode) {
        return Err(CLAUDE_CODE_UNSUPPORTED.to_owned());
    }
    if spec.harness.command.is_empty() {
        return Err("harness.command is required".to_owned());
    }
    match spec.harness.adapter {
        Adapter::ClaudeCode => {
            unreachable!("claude-code rejected above")
        }
        Adapter::Pi
            if spec
                .harness
                .command
                .first()
                .is_some_and(|command| command == "pi") =>
        {
            let model = spec
                .harness
                .model
                .as_deref()
                .ok_or_else(|| "the pi adapter requires harness.model".to_owned())?;
            validate_pi_model(model)?;
            if spec
                .harness
                .command
                .iter()
                .any(|argument| is_pi_reserved_argument(argument))
            {
                return Err(
                    "the pi adapter command must not contain dispatch or model-selection arguments"
                        .to_owned(),
                );
            }
        }
        Adapter::Pi => {
            return Err("the pi adapter requires a command beginning with `pi`".to_owned());
        }
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
    let mut domains = BTreeSet::new();
    for domain in spec
        .firewall
        .allow
        .iter()
        .chain(spec.tools.install.iter().flat_map(|tool| tool.allow.iter()))
    {
        validate_domain(domain)?;
        domains.insert(domain);
    }
    let mut required_tools = BTreeSet::new();
    for tool in &spec.tools.required {
        validate_name("required tool", tool)?;
        if !required_tools.insert(tool) {
            return Err(format!("duplicate required tool: {tool}"));
        }
    }
    let mut tool_names = BTreeSet::new();
    for tool in &spec.tools.install {
        validate_name("tool name", &tool.name)?;
        if !tool_names.insert(&tool.name) {
            return Err(format!("duplicate tool name: {}", tool.name));
        }
        validate_command("tool check", std::slice::from_ref(&tool.check))?;
        if tool.install.is_empty() {
            return Err(format!("tool install commands are required: {}", tool.name));
        }
        for command in &tool.install {
            validate_command("tool install command", command)?;
        }
    }
    if !spec.tools.install.is_empty() && matches!(spec.sandbox.network, Network::None) {
        return Err("tools require sandbox.network = 'bridge'".to_owned());
    }
    Ok(())
}

fn validate_image_profile_runtime(store: &Store, image: &ImageProfile) -> Result<()> {
    if let Some(context) = &image.source.build_context
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
fn validate_image_requirements(store: &Store, spec: &Spec) -> Result<SpecIdentity> {
    let (image, identity, _) = store.read_image(&spec.image.name)?;
    validate_image_profile_runtime(store, &image)?;
    if !image.harnesses.iter().any(|adapter| {
        std::mem::discriminant(adapter) == std::mem::discriminant(&spec.harness.adapter)
    }) {
        return Err(format!(
            "image profile {} does not support the {} harness",
            spec.image.name,
            serde_json::to_string(&spec.harness.adapter)
                .unwrap()
                .trim_matches('"')
        ));
    }
    for tool in &spec.tools.required {
        if !image.tools.contains(tool) {
            return Err(format!(
                "image profile {} does not provide required tool: {tool}",
                spec.image.name
            ));
        }
    }
    Ok(identity)
}
fn validate_runtime_spec(store: &Store, spec: &Spec) -> Result<SpecIdentity> {
    if matches!(spec.harness.adapter, Adapter::Pi) {
        validate_pi_agent(store)?;
    }
    for mount in &spec.mounts {
        approved_child(&store.mounts_path(), &mount.name)?;
    }
    validate_image_requirements(store, spec)
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

fn ensure_image(store: &Store, spec: &Spec) -> Result<(String, SpecIdentity)> {
    let (profile, identity, _) = store.read_image(&spec.image.name)?;
    validate_image_profile_runtime(store, &profile)?;
    if let Some(context) = &profile.source.build_context {
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
        Ok((image_id(&tag)?, identity))
    } else {
        let reference = profile
            .source
            .reference
            .as_deref()
            .expect("validated image profile reference");
        let inspect = Command::new("docker")
            .args(["image", "inspect", reference])
            .output()
            .map_err(io_error)?;
        if !inspect.status.success() {
            let pull = Command::new("docker")
                .args(["pull", reference])
                .output()
                .map_err(io_error)?;
            if !pull.status.success() {
                return Err(command_error("docker pull", &pull));
            }
        }
        Ok((image_id(reference)?, identity))
    }
}

fn docker_arguments(
    store: &Store,
    spec: &Spec,
    workspace: &Path,
    run_directory: &Path,
    spec_name: &str,
    run_id: &str,
    image: &str,
    harness_args: &[String],
) -> Result<Vec<String>> {
    validate_docker_path("workspace path", workspace)?;
    validate_docker_path("run directory", run_directory)?;
    let mut args = vec![
        "run".to_owned(),
        "--detach".to_owned(),
        "--name".to_owned(),
        format!("gardr-{spec_name}-{run_id}"),
        "--network".to_owned(),
        match spec.sandbox.network {
            Network::None => "none",
            Network::Bridge => "bridge",
        }
        .to_owned(),
        "--mount".to_owned(),
        format!("type=bind,source={},target=/workspace", workspace.display()),
    ];
    if matches!(spec.sandbox.network, Network::Bridge) {
        args.extend([
            "--cap-add".to_owned(),
            "NET_ADMIN".to_owned(),
            "--env".to_owned(),
            "AP_AGENT_MODE=1".to_owned(),
            "--env".to_owned(),
            "RUN_MANIFEST=/gardr/resolved.json".to_owned(),
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target=/gardr,readonly",
                run_directory.display()
            ),
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target=/usr/local/bin/init-firewall.sh,readonly",
                run_directory.join("firewall-init.sh").display()
            ),
        ]);
    }
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
    if matches!(spec.harness.adapter, Adapter::Pi) {
        let agent = store.pi_agent_path();
        validate_docker_path("Pi agent path", &agent)?;
        args.extend([
            "--env".to_owned(),
            "PI_CODING_AGENT_DIR=/pi-agent".to_owned(),
            "--env".to_owned(),
            "PI_SKIP_VERSION_CHECK=1".to_owned(),
            "--mount".to_owned(),
            format!("type=bind,source={},target=/pi-agent", agent.display()),
        ]);
    }
    for reference in &spec.credentials.environment {
        if std::env::var_os(reference).is_none() {
            return Err(format!("required credential is not set: {reference}"));
        }
        args.extend(["--env".to_owned(), reference.clone()]);
    }
    args.push(image.to_owned());
    if matches!(spec.sandbox.network, Network::Bridge) {
        args.extend([
            "sh".to_owned(),
            "/gardr/tool-bootstrap.sh".to_owned(),
            "gardr-bootstrap".to_owned(),
        ]);
    }
    let mut command = spec.harness.command.clone();
    if let Some(model) = &spec.harness.model {
        command.splice(1..1, ["--model".to_owned(), model.clone()]);
    }
    command.extend(harness_args.iter().cloned());
    args.extend(command);
    Ok(args)
}

const FIREWALL_MINIMUM: &[&str] = &[
    "api.anthropic.com",
    "api.github.com",
    "claude.ai",
    "crates.io",
    "github.com",
    "index.crates.io",
    "registry.npmjs.org",
    "static.crates.io",
    "static.rust-lang.org",
];

fn runtime_domains(spec: &Spec) -> Vec<String> {
    FIREWALL_MINIMUM
        .iter()
        .map(|domain| (*domain).to_owned())
        .chain(
            pi_runtime_domains(spec)
                .iter()
                .map(|domain| (*domain).to_owned()),
        )
        .chain(spec.firewall.allow.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn pi_runtime_domains(spec: &Spec) -> &'static [&'static str] {
    let provider = spec
        .harness
        .model
        .as_deref()
        .and_then(|model| model.split_once('/').map(|(provider, _)| provider));
    match provider {
        Some("anthropic") => &["api.anthropic.com"],
        Some("openai-codex") => &["api.openai.com", "chatgpt.com"],
        _ => &[],
    }
}

fn validate_pi_model(value: &str) -> Result<()> {
    let (provider, model) = value
        .split_once('/')
        .ok_or_else(|| "Pi model must be provider-qualified".to_owned())?;
    if provider.is_empty()
        || model.is_empty()
        || !provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/' | b':')
        })
    {
        return Err(format!("invalid Pi model: {value}"));
    }
    if !matches!(provider, "anthropic" | "openai-codex") {
        return Err(format!("unsupported Pi model provider: {provider}"));
    }
    Ok(())
}

fn sync_pi_auth(store: &Store, spec: &Spec) -> Result<()> {
    if !matches!(spec.harness.adapter, Adapter::Pi) {
        return Ok(());
    }
    if store.pi_agent_path().join("auth.json").exists() {
        return validate_pi_agent(store);
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "unable to locate host Pi authentication: HOME is not set".to_owned())?;
    sync_pi_auth_from(store, PathBuf::from(home).join(".pi/agent/auth.json"))
}

fn validate_pi_agent(store: &Store) -> Result<()> {
    let agent = store.pi_agent_path();
    let directory = fs::symlink_metadata(&agent)
        .map_err(|_| "managed Pi authentication is unavailable".to_owned())?;
    if !directory.is_dir() || directory.file_type().is_symlink() {
        return Err("managed Pi authentication directory is invalid".to_owned());
    }
    let auth = agent.join("auth.json");
    let file = fs::symlink_metadata(&auth)
        .map_err(|_| "managed Pi authentication is unavailable".to_owned())?;
    if !file.is_file() || file.file_type().is_symlink() {
        return Err("managed Pi authentication file is invalid".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if directory.permissions().mode() & 0o077 != 0 || file.permissions().mode() & 0o077 != 0 {
            return Err("managed Pi authentication permissions are not private".to_owned());
        }
    }
    Ok(())
}

fn sync_pi_auth_from(store: &Store, source: PathBuf) -> Result<()> {
    let auth = fs::read(&source).map_err(|error| {
        format!(
            "unable to read host Pi authentication {}: {error}",
            source.display()
        )
    })?;
    let agent = store.pi_agent_path();
    fs::create_dir_all(&agent).map_err(io_error)?;
    set_private_directory(&agent)?;
    let destination = agent.join("auth.json");
    atomic_write(&destination, &auth)?;
    set_private_file(&destination)
}

fn installer_domains(spec: &Spec) -> Vec<String> {
    runtime_domains(spec)
        .into_iter()
        .chain(
            spec.tools
                .install
                .iter()
                .flat_map(|tool| tool.allow.iter().cloned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn write_bootstrap(directory: &Path, spec: &Spec) -> Result<()> {
    if matches!(spec.sandbox.network, Network::None) {
        return Ok(());
    }
    let firewall = directory.join("firewall-init.sh");
    write_new(&firewall, include_bytes!("../assets/firewall-init.sh"))?;
    set_executable(&firewall)?;
    write_new(
        &directory.join("runtime-domains.txt"),
        runtime_domains(spec).join("\n").as_bytes(),
    )?;
    let mut script = String::from("#!/bin/sh\nset -eu\n");
    for tool in &spec.tools.install {
        script.push_str(&format!(
            "if ! command -v {} >/dev/null 2>&1; then\n",
            shell_quote(&tool.check)
        ));
        for command in &tool.install {
            script.push_str(
                &command
                    .iter()
                    .map(|word| shell_quote(word))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            script.push('\n');
        }
        script.push_str(&format!(
            "command -v {} >/dev/null 2>&1\nfi\n",
            shell_quote(&tool.check)
        ));
    }
    script.push_str("sudo -n /usr/local/bin/init-firewall.sh '' /workspace /gardr/runtime-domains.txt\nshift\nexec \"$@\"\n");
    let bootstrap = directory.join("tool-bootstrap.sh");
    write_new(&bootstrap, script.as_bytes())?;
    set_executable(&bootstrap)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
}
#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}
#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)
}
#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(io_error)
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
fn validate_domain(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        Err(format!("invalid firewall domain: {value}"))
    } else {
        Ok(())
    }
}
fn validate_harness_args(arguments: &[String]) -> Result<()> {
    if arguments.is_empty() {
        Ok(())
    } else {
        validate_command("harness arguments", arguments)
    }
}
fn validate_dispatch_harness_args(spec: &Spec, arguments: &[String]) -> Result<()> {
    if matches!(spec.harness.adapter, Adapter::Pi)
        && arguments
            .iter()
            .any(|argument| is_pi_model_selection_argument(argument))
    {
        return Err("Pi dispatch arguments must not select a model or provider".to_owned());
    }
    Ok(())
}
fn is_pi_reserved_argument(argument: &str) -> bool {
    matches!(argument, "-p" | "--print" | "--") || is_pi_model_selection_argument(argument)
}
fn is_pi_model_selection_argument(argument: &str) -> bool {
    matches!(
        argument,
        "--model" | "--provider" | "--api-key" | "--thinking"
    ) || ["--model=", "--provider=", "--api-key=", "--thinking="]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
}
fn validate_command(label: &str, command: &[String]) -> Result<()> {
    if command.is_empty()
        || command.iter().any(|word| {
            word.is_empty() || word.contains('\0') || word.contains('\n') || word.contains('\r')
        })
    {
        Err(format!("invalid {label}"))
    } else {
        Ok(())
    }
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
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
        b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n"
    }
    #[test]
    fn specs_are_validated_and_immutable() {
        let temp = temporary_directory();
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let store = Store::open(temp.join("store"));
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        let identity = store.add_spec("build", &source).unwrap();
        assert_eq!(identity.name, "build");
        assert_eq!(store.list_specs().unwrap(), ["build"]);
        assert!(store.add_spec("build", &source).is_err());
        assert!(store.read_spec("build").is_ok());
        fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn image_profiles_are_immutable_and_gate_harnesses_and_tools() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let profile = temp.join("agent.toml");
        fs::write(&profile, b"version = 1\nharnesses = ['pi']\ntools = ['git']\n[source]\nreference = 'example:latest'\n").unwrap();
        let identity = store.add_image("agent", &profile).unwrap();
        assert_eq!(identity.name, "agent");
        assert_eq!(store.list_images().unwrap(), ["agent"]);
        assert!(store.add_image("agent", &profile).is_err());

        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();

        let supported = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[tools]\nrequired = ['git']\n").unwrap();
        store.validate_runtime_spec(&supported).unwrap();
        let missing_tool = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[tools]\nrequired = ['go']\n").unwrap();
        assert!(
            store
                .validate_runtime_spec(&missing_tool)
                .unwrap_err()
                .contains("required tool")
        );
        // claude-code is used here (bypassing validate_spec, which now rejects it) solely to
        // exercise the image/adapter mismatch path with a second Adapter discriminant.
        let unsupported_harness = parse_spec(b"version = 1\n[image]\nname = 'agent'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'claude-code'\ncommand = ['claude']\n").unwrap();
        assert!(
            store
                .validate_image_requirements(&unsupported_harness)
                .unwrap_err()
                .contains("does not support")
        );
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
        assert!(parse_spec(b"version=1\nextra=true\n[image]\nreference='x'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['x']\n").is_err());
    }

    #[test]
    fn rejects_workspace_mount_aliases_and_unsealed_dispatches() {
        let spec = parse_spec(b"version=1\n[image]\nname='example'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['pi']\nmodel='anthropic/claude-opus-4-6'\n[[mounts]]\nname='tools'\ntarget='/workspace/.'\n").unwrap();
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

    #[test]
    fn bridge_specs_resolve_tool_installer_egress_and_bootstrap() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[firewall]\nallow = ['runtime.example']\n[[tools.install]]\nname = 'go'\ncheck = 'go'\ninstall = [['asdf', 'install', 'golang', 'latest']]\nallow = ['install.example']\n").unwrap();
        validate_spec(&spec).unwrap();
        assert_eq!(
            runtime_domains(&spec),
            vec![
                "api.anthropic.com",
                "api.github.com",
                "claude.ai",
                "crates.io",
                "github.com",
                "index.crates.io",
                "registry.npmjs.org",
                "runtime.example",
                "static.crates.io",
                "static.rust-lang.org",
            ]
        );
        assert!(installer_domains(&spec).contains(&"install.example".to_owned()));

        let temp = temporary_directory();
        write_bootstrap(&temp, &spec).unwrap();
        assert!(temp.join("firewall-init.sh").is_file());
        assert!(temp.join("runtime-domains.txt").is_file());
        assert!(
            fs::read_to_string(temp.join("tool-bootstrap.sh"))
                .unwrap()
                .contains("asdf' 'install'")
        );
        let arguments = docker_arguments(
            &Store::open(temp.join("store")),
            &spec,
            &temp,
            &temp,
            "example",
            "run-test",
            "image-id",
            &[],
        )
        .unwrap();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--cap-add", "NET_ADMIN"])
        );
        assert!(
            arguments
                .iter()
                .any(|argument| argument == "RUN_MANIFEST=/gardr/resolved.json")
        );
        let firewall = fs::read_to_string(temp.join("firewall-init.sh")).unwrap();
        assert!(firewall.contains("/etc/hosts"));
        assert!(firewall.contains("iptables -P OUTPUT DROP"));
        let bootstrap = fs::read_to_string(temp.join("tool-bootstrap.sh")).unwrap();
        assert!(bootstrap.contains("runtime-domains.txt"));
        assert!(bootstrap.contains("shift\nexec \"$@\""));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn pi_adapter_requires_a_supported_qualified_model() {
        let valid = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'openai-codex/gpt-5.5:high'\n").unwrap();
        validate_spec(&valid).unwrap();
        assert!(runtime_domains(&valid).contains(&"api.openai.com".to_owned()));
        assert!(runtime_domains(&valid).contains(&"chatgpt.com".to_owned()));

        let missing_model = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\n").unwrap();
        assert!(validate_spec(&missing_model).unwrap_err().contains("model"));
        let print_mode = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '-p']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        assert!(
            validate_spec(&print_mode)
                .unwrap_err()
                .contains("dispatch or model-selection")
        );
        let overridden_model = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--model=other']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        assert!(validate_spec(&overridden_model).is_err());
        assert!(validate_dispatch_harness_args(&valid, &["--model=other".to_owned()]).is_err());
    }

    #[test]
    fn pi_auth_is_staged_in_the_managed_directory() {
        let temp = temporary_directory();
        let source = temp.join("host-auth.json");
        fs::write(&source, b"{\"openai-codex\":{}}\n").unwrap();
        let store = Store::open(temp.join("store"));
        sync_pi_auth_from(&store, source).unwrap();
        let managed_auth = store.pi_agent_path().join("auth.json");
        assert_eq!(fs::read(&managed_auth).unwrap(), b"{\"openai-codex\":{}}\n");
        validate_pi_agent(&store).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&managed_auth).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn pi_docker_arguments_mount_managed_auth_and_select_model() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--no-session']\nmodel = 'anthropic/claude-opus-4-6:high'\n").unwrap();
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let arguments = docker_arguments(
            &store,
            &spec,
            &temp,
            &temp,
            "example",
            "run-test",
            "image-id",
            &["-p".to_owned(), "complete the assigned work".to_owned()],
        )
        .unwrap();
        assert!(
            arguments
                .iter()
                .any(|argument| argument == "PI_CODING_AGENT_DIR=/pi-agent")
        );
        assert!(
            arguments
                .iter()
                .any(|argument| argument == "PI_SKIP_VERSION_CHECK=1")
        );
        assert!(
            arguments
                .iter()
                .any(|argument| argument.contains("target=/pi-agent"))
        );
        assert!(arguments.windows(2).any(|pair| pair == ["pi", "--model"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--model", "anthropic/claude-opus-4-6:high"])
        );
        assert!(arguments.ends_with(&["-p".to_owned(), "complete the assigned work".to_owned()]));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn rejects_malformed_firewall_and_offline_tool_specs() {
        let malformed = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[firewall]\nallow = ['not/a-domain']\n").unwrap();
        assert!(
            validate_spec(&malformed)
                .unwrap_err()
                .contains("invalid firewall domain")
        );
        let offline_tool = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[[tools.install]]\nname = 'go'\ncheck = 'go'\ninstall = [['asdf', 'install', 'golang', 'latest']]\n").unwrap();
        assert!(
            validate_spec(&offline_tool)
                .unwrap_err()
                .contains("tools require")
        );
    }

    #[test]
    fn spec_add_rejects_claude_code_adapter_at_store_time() {
        let temp = temporary_directory();
        let source = temp.join("spec.toml");
        fs::write(
            &source,
            b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'claude-code'\ncommand = ['claude', '-p']\n",
        )
        .unwrap();
        let store = Store::open(temp.join("store"));
        let error = store.add_spec("build", &source).unwrap_err();
        assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
        assert!(!store.spec_path("build").unwrap().exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn image_add_rejects_claude_code_harness_at_store_time() {
        let temp = temporary_directory();
        let profile = temp.join("agent.toml");
        fs::write(
            &profile,
            b"version = 1\nharnesses = ['claude-code']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        let store = Store::open(temp.join("store"));
        let error = store.add_image("agent", &profile).unwrap_err();
        assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
        assert!(!store.image_path("agent").unwrap().exists());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn image_add_rejects_mixed_claude_code_and_pi_harnesses_at_store_time() {
        let temp = temporary_directory();
        let profile = temp.join("agent.toml");
        fs::write(
            &profile,
            b"version = 1\nharnesses = ['claude-code', 'pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        let store = Store::open(temp.join("store"));
        let error = store.add_image("agent", &profile).unwrap_err();
        assert_eq!(error, CLAUDE_CODE_UNSUPPORTED);
        assert!(!store.image_path("agent").unwrap().exists());
        fs::remove_dir_all(temp).unwrap();
    }
}

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    /// Reads the optional global config at `<root>/config.toml`. A missing file is not an error;
    /// it resolves to an all-`None` `GlobalConfig`.
    pub fn read_global_config(&self) -> Result<GlobalConfig> {
        match fs::read(self.config_path()) {
            Ok(content) => parse_global_config(&content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(GlobalConfig::default()),
            Err(error) => Err(io_error(error)),
        }
    }
    pub fn pi_agent_path(&self) -> PathBuf {
        self.root.join("pi").join("agent")
    }
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join("credentials")
    }
    pub fn credential_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("credential name", name)?;
        Ok(self.credentials_path().join(name))
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

    pub fn validate_runtime_spec(&self, spec: &Spec) -> Result<()> {
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

    /// Registers (sets/upserts) a credential value under the private store. The value is never
    /// echoed back by any command.
    pub fn set_credential(&self, name: &str, value: &[u8]) -> Result<()> {
        validate_credential_value(value)?;
        let path = self.credential_path(name)?;
        create_private_dir_all(&self.credentials_path())?;
        atomic_write(&path, value)
    }

    /// Lists registered credential names. Values are never included.
    pub fn list_credentials(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !self.credentials_path().exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(self.credentials_path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            if entry.file_type().map_err(io_error)?.is_file() {
                names.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn remove_credential(&self, name: &str) -> Result<()> {
        let path = self.credential_path(name)?;
        if !path.exists() {
            return Err(format!("credential does not exist: {name}"));
        }
        fs::remove_file(&path).map_err(io_error)
    }

    pub fn create_run(
        &self,
        overrides: RuntimeOverrides,
        spec_name: Option<&str>,
        harness_args: Vec<String>,
    ) -> Result<(RunRecord, Spec)> {
        validate_harness_args(&harness_args)?;
        let global = self.read_global_config()?;
        let spec_name = spec_name.map(str::to_owned).or_else(|| global.spec.clone());
        let (spec_source, identity, content) = match &spec_name {
            Some(name) => {
                let (spec, identity, content) = self.read_spec(name)?;
                (Some(spec), Some(identity), Some(content))
            }
            None => (None, None, None),
        };
        let effective = resolve_runtime(&global, spec_source.as_ref(), &overrides)?;
        let spec = apply_effective_runtime(spec_source.unwrap_or_else(implicit_spec), &effective);
        validate_dispatch_harness_args(&spec, &harness_args)?;
        sync_pi_auth(self, &spec)?;
        validate_resolved_spec(self, &spec)?;
        let workspace = validate_workspace(Path::new(&effective.workspace.value))?;
        fs::create_dir_all(self.runs_path()).map_err(io_error)?;
        let id = new_run_id();
        let directory = self.run_path(&id)?;
        fs::create_dir(&directory).map_err(io_error)?;
        let record = RunRecord {
            version: 1,
            id: id.clone(),
            state: RunState::Prepared,
            spec: identity,
            effective: effective.clone(),
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
            transcript_path: matches!(spec.harness.adapter, Some(Adapter::Pi)).then(|| {
                directory
                    .join("transcript")
                    .join("session.jsonl")
                    .display()
                    .to_string()
            }),
            stdout_path: directory.join("stdout.log").display().to_string(),
            stderr_path: directory.join("stderr.log").display().to_string(),
            usage: None,
            usage_error: None,
        };
        if let Some(content) = &content {
            write_new(&directory.join("spec.toml"), content)?;
        }
        if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
            fs::create_dir(directory.join("transcript")).map_err(io_error)?;
        }
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
        // `resume` never re-merges the global/spec/CLI layers: it uses the frozen effective
        // values recorded at `run start` and only reloads the frozen spec.toml (if any) for the
        // static, unlayered fields (sandbox, mounts, credentials, firewall, tools).
        let workspace = validate_workspace(Path::new(&record.effective.workspace.value))?;
        let baseline = match &record.spec {
            Some(identity) => {
                let frozen = fs::read(directory.join("spec.toml")).map_err(io_error)?;
                if digest(&frozen) != identity.sha256 {
                    return Err("frozen run spec does not match recorded spec identity".to_owned());
                }
                let spec = parse_spec(&frozen)?;
                validate_spec(&spec)?;
                spec
            }
            None => implicit_spec(),
        };
        let spec = apply_effective_runtime(baseline, &record.effective);
        validate_resolved_spec(self, &spec)?;
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
        overrides: RuntimeOverrides,
        spec_name: Option<&str>,
        harness_args: Vec<String>,
    ) -> Result<RunRecord> {
        let (record, spec) = self.create_run(overrides, spec_name, harness_args)?;
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
        record.usage = None;
        record.usage_error = None;
        if let Some(path) = record.transcript_path.as_deref() {
            match read_pi_transcript_usage(Path::new(path)) {
                Ok((usage, skipped)) => {
                    record.usage = usage;
                    if skipped > 0 {
                        record.usage_error = Some(format!(
                            "skipped {skipped} unparsable transcript line(s); totals may be incomplete"
                        ));
                    }
                }
                Err(error) => record.usage_error = Some(error),
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
        if let Some(container) = &record.container {
            let stdout_path = if record.stdout_path.is_empty() {
                directory.join("stdout.log")
            } else {
                PathBuf::from(&record.stdout_path)
            };
            let stderr_path = if record.stderr_path.is_empty() {
                directory.join("stderr.log")
            } else {
                PathBuf::from(&record.stderr_path)
            };
            // Best-effort: an ordinary `docker logs` failure (unreadable log driver, transient
            // daemon hiccup, permissions) must not block `docker rm` or leave the run stuck
            // short of a terminal cleaned state.
            if let Err(error) = capture_container_logs(container, &stdout_path, &stderr_path) {
                append_log(&directory, &format!("log capture failed: {error}"))?;
            }
        }
        if let Some(container) = record.container {
            let output = Command::new("docker")
                .args(["rm", &container])
                .output()
                .map_err(io_error)?;
            if !output.status.success() {
                if String::from_utf8_lossy(&output.stderr).contains("No such container") {
                    append_log(
                        &directory,
                        "docker rm: container already removed; capture files may be absent",
                    )?;
                } else {
                    return Err(command_error("docker rm", &output));
                }
            }
        }
        remove_credentials_env(&directory);
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
        let spec_label = record
            .spec
            .as_ref()
            .map(|identity| identity.name.as_str())
            .unwrap_or("adhoc");
        let arguments = match docker_arguments(
            self,
            &spec,
            &workspace,
            &directory,
            spec_label,
            &record.id,
            &image,
            &record.harness_args,
        ) {
            Ok(arguments) => arguments,
            Err(error) => return fail_run(&directory, &mut record, error),
        };
        let output = match Command::new("docker").args(&arguments).output() {
            Ok(output) => output,
            Err(error) => {
                remove_credentials_env(&directory);
                return fail_run(&directory, &mut record, io_error(error));
            }
        };
        // Docker only reads `--env-file` at this invocation, not afterward; the plaintext
        // resolved-secrets file has no further purpose and shouldn't outlive the run in cleartext.
        remove_credentials_env(&directory);
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

/// Best-effort removal of the run's plaintext resolved-credentials env-file. Missing is not an
/// error (e.g. the spec has no credentials, or cleanup already removed it).
fn remove_credentials_env(directory: &Path) {
    let path = directory.join("credentials.env");
    if let Err(error) = fs::remove_file(&path)
        && error.kind() != io::ErrorKind::NotFound
    {
        let _ = append_log(
            directory,
            &format!("failed to remove {}: {error}", path.display()),
        );
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
    /// The default workspace path for this spec, layered under global config and overridden by
    /// `run start --workspace`. Rarely set; most specs leave workspace selection to the CLI.
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub image: Image,
    pub sandbox: Sandbox,
    #[serde(default)]
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(default)]
    pub name: Option<String>,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    #[serde(default)]
    pub adapter: Option<Adapter>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
    ClaudeCode,
    Pi,
}
impl std::str::FromStr for Adapter {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "pi" => Ok(Adapter::Pi),
            "claude-code" => Ok(Adapter::ClaudeCode),
            other => Err(format!("unknown harness adapter: {other}")),
        }
    }
}
impl std::fmt::Display for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap().trim_matches('"')
        )
    }
}
pub const CLAUDE_CODE_UNSUPPORTED: &str = "the claude-code adapter is not supported: it has no credential bootstrap yet; use the pi adapter with an Anthropic model instead";
/// Container path where the pi adapter's writable transcript directory is mounted.
const PI_TRANSCRIPT_MOUNT: &str = "/gardr-transcript";

/// Default `harness.command` per adapter, used when neither a spec nor the global config sets
/// one.
fn default_harness_command(adapter: Adapter) -> Vec<String> {
    match adapter {
        Adapter::Pi => vec!["pi".to_owned(), "--no-session".to_owned()],
        Adapter::ClaudeCode => vec!["claude".to_owned()],
    }
}

/// The optional global config at `<root>/config.toml`. Every key is a default consulted only when
/// the spec and CLI layers leave it unset; a missing file resolves to every field `None`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// Default workspace path, overridden by a spec's `workspace` and by `run start --workspace`.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Default spec name, overridden by `run start --spec`.
    #[serde(default)]
    pub spec: Option<String>,
    /// Default image profile name, overridden by a spec's `image.name` and `run start --image`.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub harness: Option<GlobalHarness>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalHarness {
    /// Default harness adapter, overridden by a spec's `harness.adapter` and `run start --harness`.
    #[serde(default)]
    pub adapter: Option<Adapter>,
    /// Default `harness.command`, overridden only by a spec's `harness.command` (there is no CLI
    /// override); falls back to `default_harness_command` when unset everywhere.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Default harness model, overridden by a spec's `harness.model` and `run start --model`.
    #[serde(default)]
    pub model: Option<String>,
}
pub fn parse_global_config(content: &[u8]) -> Result<GlobalConfig> {
    toml::from_str(
        std::str::from_utf8(content)
            .map_err(|error| format!("global config is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("invalid global config: {error}"))
}

/// The `run start` command-line overrides for the four layered runtime keys. Each, if present,
/// wins over the spec and global config layers for that key.
#[derive(Clone, Debug, Default)]
pub struct RuntimeOverrides {
    pub workspace: Option<String>,
    pub image: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
}

/// Which layer supplied a resolved runtime value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Cli,
    Spec,
    Global,
    /// No layer set the key; an adapter-specific built-in default was used. Only ever the source
    /// for `harness_command`.
    Default,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resolved<T> {
    pub value: T,
    pub source: Layer,
}
/// The fully merged runtime configuration for one run: the effective value of each of the four
/// layered keys (`workspace`, `image`, `harness`, `model`), plus `harness_command`, and which
/// layer supplied each. Frozen into the run record at `run start`; `resume` uses it verbatim and
/// never re-merges.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectiveRuntime {
    pub workspace: Resolved<String>,
    pub image: Resolved<String>,
    pub harness: Resolved<Adapter>,
    pub model: Resolved<String>,
    pub harness_command: Resolved<Vec<String>>,
}

/// Merges one required layered key: CLI overrides the spec, which overrides the global config.
/// Fails explicitly, naming the key and the layers consulted, when no layer set it.
fn merge_required(
    key: &str,
    cli: Option<String>,
    spec: Option<String>,
    global: Option<String>,
) -> Result<(String, Layer)> {
    if let Some(value) = cli {
        return Ok((value, Layer::Cli));
    }
    if let Some(value) = spec {
        return Ok((value, Layer::Spec));
    }
    if let Some(value) = global {
        return Ok((value, Layer::Global));
    }
    Err(format!(
        "missing required configuration key `{key}`: checked cli, spec, and global layers"
    ))
}
/// Merges `harness.command`: spec overrides global config; there is no CLI override. Falls back
/// to the resolved adapter's built-in default when neither layer sets it.
fn merge_harness_command(
    spec: Option<Vec<String>>,
    global: Option<Vec<String>>,
    adapter: Adapter,
) -> (Vec<String>, Layer) {
    if let Some(command) = spec {
        return (command, Layer::Spec);
    }
    if let Some(command) = global {
        return (command, Layer::Global);
    }
    (default_harness_command(adapter), Layer::Default)
}
/// Merges the global config, an optional spec, and `run start` CLI overrides into one
/// `EffectiveRuntime`, per-key precedence CLI > spec > global. Fails explicitly, naming the key
/// and the layers consulted, when a required key resolves to nothing.
fn resolve_runtime(
    global: &GlobalConfig,
    spec: Option<&Spec>,
    overrides: &RuntimeOverrides,
) -> Result<EffectiveRuntime> {
    let global_harness = global.harness.as_ref();
    let workspace = merge_required(
        "workspace",
        overrides.workspace.clone(),
        spec.and_then(|spec| spec.workspace.clone()),
        global.workspace.clone(),
    )?;
    let image = merge_required(
        "image",
        overrides.image.clone(),
        spec.and_then(|spec| spec.image.name.clone()),
        global.image.clone(),
    )?;
    let (harness_name, harness_source) = merge_required(
        "harness",
        overrides.harness.clone(),
        spec.and_then(|spec| spec.harness.adapter)
            .map(|adapter| adapter.to_string()),
        global_harness
            .and_then(|harness| harness.adapter)
            .map(|adapter| adapter.to_string()),
    )?;
    let adapter = harness_name.parse::<Adapter>()?;
    let model = merge_required(
        "model",
        overrides.model.clone(),
        spec.and_then(|spec| spec.harness.model.clone()),
        global_harness.and_then(|harness| harness.model.clone()),
    )?;
    let harness_command = merge_harness_command(
        spec.map(|spec| spec.harness.command.clone())
            .filter(|command| !command.is_empty()),
        global_harness
            .and_then(|harness| harness.command.clone())
            .filter(|command| !command.is_empty()),
        adapter,
    );
    Ok(EffectiveRuntime {
        workspace: Resolved {
            value: workspace.0,
            source: workspace.1,
        },
        image: Resolved {
            value: image.0,
            source: image.1,
        },
        harness: Resolved {
            value: adapter,
            source: harness_source,
        },
        model: Resolved {
            value: model.0,
            source: model.1,
        },
        harness_command: Resolved {
            value: harness_command.0,
            source: harness_command.1,
        },
    })
}
/// The neutral baseline used when a run has no spec at all: no image/harness/model of its own (all
/// filled in by `apply_effective_runtime`), no mounts/credentials/tools, and network `none` since
/// firewall policy has no layering of its own yet (a later ticket moves it to global-only config).
fn implicit_spec() -> Spec {
    Spec {
        version: 1,
        workspace: None,
        image: Image::default(),
        sandbox: Sandbox {
            network: Network::None,
        },
        harness: Harness::default(),
        mounts: Vec::new(),
        credentials: Credentials::default(),
        firewall: Firewall::default(),
        tools: Tooling::default(),
    }
}
/// Overwrites a baseline spec's layered fields (`image.name`, `harness.adapter`,
/// `harness.command`, `harness.model`) with the merged effective values, leaving every unlayered
/// field (sandbox, mounts, credentials, firewall, tools) exactly as the baseline declared it.
fn apply_effective_runtime(mut spec: Spec, effective: &EffectiveRuntime) -> Spec {
    spec.image.name = Some(effective.image.value.clone());
    spec.harness.adapter = Some(effective.harness.value);
    spec.harness.command = effective.harness_command.value.clone();
    spec.harness.model = Some(effective.model.value.clone());
    spec
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
    pub environment: Vec<CredentialRef>,
}
/// One `[credentials] environment` entry: either a bare env-var name (backward-compatible plain
/// string form) or a table mapping an in-container env var `name` to a credential-store key
/// `from` (defaulting to `name` when omitted).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CredentialRef {
    Name(String),
    Mapped(MappedCredential),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappedCredential {
    pub name: String,
    #[serde(default)]
    pub from: Option<String>,
}
impl CredentialRef {
    /// The in-container environment variable name.
    pub fn env_name(&self) -> &str {
        match self {
            CredentialRef::Name(name) => name,
            CredentialRef::Mapped(mapped) => &mapped.name,
        }
    }
    /// The credential-store key to resolve the value from; defaults to `env_name()`.
    pub fn registry_key(&self) -> &str {
        match self {
            CredentialRef::Name(name) => name,
            CredentialRef::Mapped(mapped) => mapped.from.as_deref().unwrap_or(&mapped.name),
        }
    }
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
    /// The stored spec's identity, or `None` when the run resolved entirely from global config
    /// and CLI overrides with no `--spec`.
    pub spec: Option<SpecIdentity>,
    /// The merged runtime configuration frozen at `run start`. `resume` uses these values
    /// verbatim and never re-merges the global config, spec, or CLI layers.
    pub effective: EffectiveRuntime,
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
    /// Path under this run's directory where the pi adapter's session transcript is written.
    /// `None` for adapters without transcript support.
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Path under this run's directory holding the container's captured raw stdout, written
    /// during cleanup before `docker rm`.
    #[serde(default)]
    pub stdout_path: String,
    /// Path under this run's directory holding the container's captured raw stderr, written
    /// during cleanup before `docker rm`.
    #[serde(default)]
    pub stderr_path: String,
    /// Cost/usage recovered from the harness transcript, when available. Computed live from
    /// `transcript_path`; never persisted to `state.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<HarnessUsage>,
    /// Set when the transcript could not be read or parsed cleanly (permission denied, non-UTF8,
    /// other IO failure, or unparsable lines beyond the tolerated in-progress trailing line).
    /// Distinguishes "couldn't read/parse the transcript" from "no spend yet" (`usage: None`
    /// with `usage_error: None`). Computed live; never persisted to `state.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_cost: f64,
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
    spec: &'a Option<SpecIdentity>,
    effective: &'a EffectiveRuntime,
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
            effective: &record.effective,
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
            pi_agent: matches!(spec.harness.adapter, Some(Adapter::Pi)).then(|| ResolvedPiAgent {
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
/// Validates only the values a spec itself sets. `image.name`, `harness.adapter`, `harness.model`,
/// and `harness.command` may all be absent (deferred to the global config or `run start` CLI
/// flags); whatever the spec DOES set must be individually valid. Completeness of the merged
/// result (all four resolved, mutually consistent, and matched by an image profile) is checked
/// only at run resolve time, against the merged result — see `validate_resolved_harness`.
pub fn validate_spec(spec: &Spec) -> Result<()> {
    if spec.version != 1 {
        return Err("run spec version must be 1".to_owned());
    }
    if let Some(name) = &spec.image.name {
        validate_name("image name", name)?;
    }
    if matches!(spec.harness.adapter, Some(Adapter::ClaudeCode)) {
        return Err(CLAUDE_CODE_UNSUPPORTED.to_owned());
    }
    if !spec.harness.command.is_empty() {
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
        if matches!(spec.harness.adapter, Some(Adapter::Pi))
            && spec
                .harness
                .command
                .first()
                .is_some_and(|command| command != "pi")
        {
            return Err("the pi adapter requires a command beginning with `pi`".to_owned());
        }
    }
    if let Some(model) = &spec.harness.model {
        validate_pi_model(model)?;
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
    let mut credential_names = BTreeSet::new();
    for reference in &spec.credentials.environment {
        validate_environment_reference(reference.env_name())?;
        validate_name("credential registry key", reference.registry_key())?;
        if !credential_names.insert(reference.env_name()) {
            return Err(format!(
                "duplicate credential environment name: {}",
                reference.env_name()
            ));
        }
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
/// Cross-checks the merged result's `image` against the image profile store: the profile must
/// exist, support the resolved `harness` adapter, and provide every `tools.required` capability.
/// Callers must pass a spec whose `image.name` and `harness.adapter` are already resolved
/// (`Some`); this is only meaningful against a fully merged runtime, never a spec in isolation.
fn validate_image_requirements(store: &Store, spec: &Spec) -> Result<SpecIdentity> {
    let image_name = spec
        .image
        .name
        .as_deref()
        .ok_or_else(|| "missing required configuration key `image`".to_owned())?;
    let adapter = spec
        .harness
        .adapter
        .as_ref()
        .ok_or_else(|| "missing required configuration key `harness`".to_owned())?;
    let (image, identity, _) = store.read_image(image_name)?;
    validate_image_profile_runtime(store, &image)?;
    if !image
        .harnesses
        .iter()
        .any(|candidate| std::mem::discriminant(candidate) == std::mem::discriminant(adapter))
    {
        return Err(format!(
            "image profile {} does not support the {} harness",
            image_name,
            serde_json::to_string(adapter).unwrap().trim_matches('"')
        ));
    }
    for tool in &spec.tools.required {
        if !image.tools.contains(tool) {
            return Err(format!(
                "image profile {image_name} does not provide required tool: {tool}"
            ));
        }
    }
    Ok(identity)
}
/// Validates a spec's own, unlayered runtime dependencies: the managed Pi authentication
/// directory (if the resolved adapter is `pi`) and that declared mounts name approved store
/// entries. Does not require `image`/`harness.model` to be resolved, so it stays usable against a
/// spec that hasn't been merged with global config or CLI overrides yet (e.g. `spec validate`).
fn validate_runtime_spec(store: &Store, spec: &Spec) -> Result<()> {
    if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
        validate_pi_agent(store)?;
    }
    for mount in &spec.mounts {
        approved_child(&store.mounts_path(), &mount.name)?;
    }
    Ok(())
}
/// Validates that the merged `harness` (adapter, command, model) is complete and internally
/// consistent: the resolved fields formerly checked only at `spec add` — supported adapter,
/// command shape, provider-qualified model — now checked against the merged result.
fn validate_resolved_harness(harness: &Harness) -> Result<()> {
    let adapter = harness
        .adapter
        .as_ref()
        .ok_or_else(|| "missing required configuration key `harness`".to_owned())?;
    if matches!(adapter, Adapter::ClaudeCode) {
        return Err(CLAUDE_CODE_UNSUPPORTED.to_owned());
    }
    if harness.command.is_empty() {
        return Err("harness.command is required".to_owned());
    }
    match adapter {
        Adapter::ClaudeCode => unreachable!("claude-code rejected above"),
        Adapter::Pi
            if harness
                .command
                .first()
                .is_some_and(|command| command == "pi") =>
        {
            let model = harness
                .model
                .as_deref()
                .ok_or_else(|| "missing required configuration key `model`".to_owned())?;
            validate_pi_model(model)?;
            if harness
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
    Ok(())
}
/// Runs every validation formerly performed only at `spec add` — supported adapter, image profile
/// provides the harness, provider-qualified model, `tools.required` satisfied by the image —
/// against the merged result, plus the spec's own unlayered runtime dependencies (mounts, managed
/// Pi authentication). Called at `run start`/`run resume`, never at `spec add`.
fn validate_resolved_spec(store: &Store, spec: &Spec) -> Result<()> {
    validate_resolved_harness(&spec.harness)?;
    validate_runtime_spec(store, spec)?;
    validate_image_requirements(store, spec)?;
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
    Ok(workspace)
}

fn ensure_image(store: &Store, spec: &Spec) -> Result<(String, SpecIdentity)> {
    let image_name = spec
        .image
        .name
        .as_deref()
        .expect("ensure_image called against a resolved spec");
    let (profile, identity, _) = store.read_image(image_name)?;
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
    if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
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
        let transcript_directory = run_directory.join("transcript");
        validate_docker_path("Pi transcript path", &transcript_directory)?;
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target={}",
                transcript_directory.display(),
                PI_TRANSCRIPT_MOUNT
            ),
        ]);
    }
    if !spec.credentials.environment.is_empty() {
        let mut lines = String::new();
        for reference in &spec.credentials.environment {
            let key = reference.registry_key();
            let path = store.credential_path(key)?;
            let value = fs::read(&path).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    format!("required credential is not registered: {key}")
                } else {
                    io_error(error)
                }
            })?;
            let value = std::str::from_utf8(&value)
                .map_err(|error| format!("registered credential {key} is not UTF-8: {error}"))?;
            let value = value.strip_suffix('\n').unwrap_or(value);
            lines.push_str(reference.env_name());
            lines.push('=');
            lines.push_str(value);
            lines.push('\n');
        }
        let env_file = run_directory.join("credentials.env");
        atomic_write(&env_file, lines.as_bytes())?;
        args.extend(["--env-file".to_owned(), env_file.display().to_string()]);
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
    command.extend(harness_args.iter().cloned());
    if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
        // Gardr manages the pi transcript itself; a reusable `command` may still carry
        // `--no-session` (e.g. today's example specs), so drop it rather than requiring specs
        // to be rewritten.
        command.retain(|argument| argument != "--no-session");
    }
    let mut injected = Vec::new();
    if let Some(model) = &spec.harness.model {
        injected.push("--model".to_owned());
        injected.push(model.clone());
    }
    if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
        injected.push("--session".to_owned());
        injected.push(format!("{PI_TRANSCRIPT_MOUNT}/session.jsonl"));
    }
    command.splice(1..1, injected);
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
    if !matches!(spec.harness.adapter, Some(Adapter::Pi)) {
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
/// Creates `path` (and any missing parents) with mode 0700 set from the `mkdir` call itself,
/// rather than a normal `create_dir_all` followed by a later `chmod`, so a directory that will
/// hold secret material is never briefly world/group-accessible at the umask default.
#[cfg(unix)]
fn create_private_dir_all(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(io_error)
}
#[cfg(not(unix))]
fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(io_error)
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
    // Append (not replace) a suffix that includes the current process ID: `validate_name` allows
    // `.` in stored names (e.g. a credential named `a.b`), so `path.with_extension("new")` could
    // silently collide with and clobber an unrelated sibling entry (e.g. `a`). Appending to the
    // full file name instead keeps the temp path derived from, but distinct from, every valid
    // stored name.
    let mut temporary_name = path
        .file_name()
        .expect("atomic_write path has a file name")
        .to_os_string();
    temporary_name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    write_private(&temporary, contents)?;
    fs::rename(temporary, path).map_err(io_error)
}
/// Writes `contents` to a newly created `path` with mode 0600 set by the `open` call itself
/// (rather than a normal write followed by a later `chmod`), so files that may hold secret
/// material (credentials, the run's resolved `credentials.env`) are never briefly readable at the
/// umask default. `atomic_write`'s callers that don't hold secrets (spec/run state, etc.) are
/// unaffected in practice other than also becoming private, which is harmless.
#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)?;
    file.write_all(contents).map_err(io_error)
}
#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).map_err(io_error)
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
/// Captures a container's raw stdout/stderr via `docker logs`, streaming directly into the two
/// destination files (rather than buffering the whole output in memory) so capture is incremental
/// and doesn't risk OOM on a chatty run.
fn capture_container_logs(container: &str, stdout_path: &Path, stderr_path: &Path) -> Result<()> {
    let stdout_file = File::create(stdout_path).map_err(io_error)?;
    let stderr_file = File::create(stderr_path).map_err(io_error)?;
    let status = Command::new("docker")
        .args(["logs", container])
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .status()
        .map_err(io_error)?;
    if !status.success() {
        let stderr = fs::read_to_string(stderr_path).unwrap_or_default();
        if stderr.contains("No such container") {
            return Ok(());
        }
        return Err(format!("docker logs failed: {}", stderr.trim()));
    }
    Ok(())
}

/// Sums usage/cost entries from a pi session transcript. Returns `Ok((None, 0))` when the
/// transcript doesn't exist yet (no spend to report) or exists but has no usage entries. Returns
/// `Err` when the transcript exists but could not be read cleanly (permission denied, non-UTF8,
/// other IO failure) so callers can tell "no spend yet" apart from "couldn't read the transcript".
/// The final line is tolerated if unparsable (an in-progress transcript may have a torn trailing
/// write); any other unparsable line is counted in the returned skipped-entry count rather than
/// silently dropped, since a partial number shouldn't be presented as authoritative.
fn read_pi_transcript_usage(path: &Path) -> Result<(Option<HarnessUsage>, u64)> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((None, 0)),
        Err(error) => {
            return Err(format!(
                "could not read pi transcript {}: {error}",
                path.display()
            ));
        }
    };
    let mut usage = HarnessUsage::default();
    let mut seen = false;
    let mut skipped = 0u64;
    let lines: Vec<&str> = content.lines().collect();
    let last_index = lines.len().saturating_sub(1);
    for (index, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(entry) => entry,
            Err(_) => {
                if index != last_index {
                    skipped += 1;
                }
                continue;
            }
        };
        let usage_value = match entry.get("type").and_then(|value| value.as_str()) {
            Some("message") => entry
                .get("message")
                .and_then(|message| message.get("usage")),
            Some("compaction") | Some("branch_summary") => entry.get("usage"),
            _ => None,
        };
        if let Some(usage_value) = usage_value {
            seen = true;
            usage.input_tokens += usage_value
                .get("input")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.output_tokens += usage_value
                .get("output")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.cache_read_tokens += usage_value
                .get("cacheRead")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            usage.cache_write_tokens += usage_value
                .get("cacheWrite")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if let Some(cost) = usage_value.get("cost") {
                usage.total_cost += cost
                    .get("total")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0);
            }
        }
    }
    Ok((seen.then_some(usage), skipped))
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
/// Rejects credential values that would be unsafe to write verbatim into an `--env-file`'s
/// `NAME=value` line: Docker splits `--env-file` on newlines and treats a leading `#` as a
/// comment, with no quoting support, so an embedded newline (e.g. a pasted PEM/SSH key) would
/// inject additional, attacker/user-uncontrolled env assignments into the container and truncate
/// the real secret. A single trailing newline (common from files written by editors) is tolerated
/// since it is stripped before the value is written to the env file.
fn validate_credential_value(value: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(value)
        .map_err(|error| format!("credential value is not UTF-8: {error}"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.contains('\n') {
        return Err(
            "credential value must be a single line: multi-line values (e.g. a pasted PEM/SSH \
             key) cannot be safely stored in an env-file"
                .to_owned(),
        );
    }
    if text.starts_with('#') {
        return Err(
            "credential value must not begin with '#': env-files treat a leading '#' as a comment"
                .to_owned(),
        );
    }
    Ok(())
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
    if matches!(spec.harness.adapter, Some(Adapter::Pi))
        && arguments.iter().any(|argument| {
            is_pi_model_selection_argument(argument) || is_pi_session_management_argument(argument)
        })
    {
        return Err(
            "Pi dispatch arguments must not select a model, provider, or session".to_owned(),
        );
    }
    Ok(())
}
fn is_pi_reserved_argument(argument: &str) -> bool {
    matches!(argument, "-p" | "--print" | "--")
        || is_pi_model_selection_argument(argument)
        || is_pi_session_management_argument(argument)
}
fn is_pi_model_selection_argument(argument: &str) -> bool {
    matches!(
        argument,
        "--model" | "--provider" | "--api-key" | "--thinking"
    ) || ["--model=", "--provider=", "--api-key=", "--thinking="]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
}
/// Session-management arguments Gardr must own for the pi adapter, since it manages the
/// transcript itself. `--no-session` is deliberately excluded: existing specs may still declare
/// it in `harness.command` and Gardr silently drops it rather than requiring a rewrite.
///
/// `--name`/`-n` (session display name) is deliberately excluded too: it sets a label on the
/// session Gardr already selects via the injected `--session`, and does not conflict with it.
/// Rejecting it would break specs that were valid before Gardr started injecting `--session`.
fn is_pi_session_management_argument(argument: &str) -> bool {
    matches!(
        argument,
        "--session"
            | "--session-id"
            | "--session-dir"
            | "--fork"
            | "--continue"
            | "-c"
            | "--resume"
            | "-r"
    ) || ["--session=", "--session-id=", "--session-dir=", "--fork="]
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
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static PATH_LOCK: Mutex<()> = Mutex::new(());

    /// Installs a fake `docker` executable at the front of `PATH` for the duration of the guard,
    /// restoring the previous `PATH` on drop. Serialized via `PATH_LOCK` since `PATH` is
    /// process-global.
    struct FakeDocker {
        _guard: std::sync::MutexGuard<'static, ()>,
        original_path: Option<std::ffi::OsString>,
    }
    impl FakeDocker {
        fn install(script: &str, bin_dir: &Path) -> Self {
            let guard = PATH_LOCK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            fs::create_dir_all(bin_dir).unwrap();
            let docker = bin_dir.join("docker");
            fs::write(&docker, script).unwrap();
            set_executable(&docker).unwrap();
            let original_path = std::env::var_os("PATH");
            let mut new_path = bin_dir.as_os_str().to_owned();
            if let Some(existing) = &original_path {
                new_path.push(":");
                new_path.push(existing);
            }
            unsafe {
                std::env::set_var("PATH", new_path);
            }
            Self {
                _guard: guard,
                original_path,
            }
        }
    }
    impl Drop for FakeDocker {
        fn drop(&mut self) {
            unsafe {
                match &self.original_path {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }
    /// Builds `RuntimeOverrides` supplying only `--workspace`, for tests exercising a spec that
    /// already sets image/harness/model itself.
    fn overrides_for(workspace: &Path) -> RuntimeOverrides {
        RuntimeOverrides {
            workspace: Some(workspace.display().to_string()),
            ..Default::default()
        }
    }
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
        // `tools.required` satisfaction is checked against the image profile store at run
        // resolve time (`validate_image_requirements`), not by `validate_runtime_spec`, which
        // only checks a spec's own unlayered runtime dependencies (mounts, managed Pi auth).
        assert!(
            store
                .validate_image_requirements(&missing_tool)
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
    fn workspace_validation_accepts_arbitrary_directory_contents() {
        let temp = temporary_directory();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("arbitrary-input"), b"content").unwrap();
        assert!(validate_workspace(&workspace).unwrap().is_dir());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn workspace_validation_rejects_non_directories() {
        let temp = temporary_directory();
        let file = temp.join("workspace");
        fs::write(&file, b"not a directory").unwrap();
        assert!(validate_workspace(&file).unwrap_err().contains("directory"));
        assert!(validate_workspace(&temp.join("missing")).is_err());
        fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn rejects_unapproved_spec_fields() {
        assert!(parse_spec(b"version=1\nextra=true\n[image]\nreference='x'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['x']\n").is_err());
    }

    #[test]
    fn rejects_workspace_mount_aliases() {
        let spec = parse_spec(b"version=1\n[image]\nname='example'\n[sandbox]\nnetwork='none'\n[harness]\nadapter='pi'\ncommand=['pi']\nmodel='anthropic/claude-opus-4-6'\n[[mounts]]\nname='tools'\ntarget='/workspace/.'\n").unwrap();
        assert!(validate_spec(&spec).is_err());
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

        // A spec no longer has to set harness.model itself: it may be deferred to the global
        // config or `run start --model`. `validate_spec` (spec add) only validates a model that
        // IS present; requiring one to be present after merging is `validate_resolved_harness`'s
        // job, exercised in the layered-config resolution tests.
        let missing_model = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'bridge'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\n").unwrap();
        assert!(validate_spec(&missing_model).is_ok());
        assert!(
            validate_resolved_harness(&missing_model.harness)
                .unwrap_err()
                .contains("model")
        );
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
    fn resolve_runtime_precedence_is_cli_over_spec_over_global() {
        let global = GlobalConfig {
            workspace: Some("/global/workspace".to_owned()),
            image: Some("global-image".to_owned()),
            harness: Some(GlobalHarness {
                adapter: Some(Adapter::Pi),
                command: Some(vec!["pi".to_owned(), "--global".to_owned()]),
                model: Some("anthropic/global-model".to_owned()),
            }),
            spec: Some("global-spec".to_owned()),
        };
        let spec = parse_spec(b"version = 1\nworkspace = '/spec/workspace'\n[image]\nname = 'spec-image'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--spec-flag']\nmodel = 'anthropic/spec-model'\n").unwrap();

        // No CLI overrides: every key comes from the spec, since the spec sets all of them.
        let from_spec =
            resolve_runtime(&global, Some(&spec), &RuntimeOverrides::default()).unwrap();
        assert_eq!(from_spec.workspace.value, "/spec/workspace");
        assert_eq!(from_spec.workspace.source, Layer::Spec);
        assert_eq!(from_spec.image.value, "spec-image");
        assert_eq!(from_spec.image.source, Layer::Spec);
        assert_eq!(from_spec.harness.value, Adapter::Pi);
        assert_eq!(from_spec.harness.source, Layer::Spec);
        assert_eq!(from_spec.model.value, "anthropic/spec-model");
        assert_eq!(from_spec.model.source, Layer::Spec);
        assert_eq!(from_spec.harness_command.value, vec!["pi", "--spec-flag"]);
        assert_eq!(from_spec.harness_command.source, Layer::Spec);

        // CLI overrides win over the spec, which still wins over the global config.
        let overrides = RuntimeOverrides {
            workspace: Some("/cli/workspace".to_owned()),
            image: Some("cli-image".to_owned()),
            harness: None,
            model: Some("anthropic/cli-model".to_owned()),
        };
        let merged = resolve_runtime(&global, Some(&spec), &overrides).unwrap();
        assert_eq!(merged.workspace.value, "/cli/workspace");
        assert_eq!(merged.workspace.source, Layer::Cli);
        assert_eq!(merged.image.value, "cli-image");
        assert_eq!(merged.image.source, Layer::Cli);
        assert_eq!(merged.model.value, "anthropic/cli-model");
        assert_eq!(merged.model.source, Layer::Cli);
        // harness (adapter) has no CLI override here, so it falls back to the spec.
        assert_eq!(merged.harness.value, Adapter::Pi);
        assert_eq!(merged.harness.source, Layer::Spec);

        // With no spec at all, every key falls back to the global config.
        let global_only = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap();
        assert_eq!(global_only.workspace.value, "/global/workspace");
        assert_eq!(global_only.workspace.source, Layer::Global);
        assert_eq!(global_only.image.value, "global-image");
        assert_eq!(global_only.image.source, Layer::Global);
        assert_eq!(global_only.harness.value, Adapter::Pi);
        assert_eq!(global_only.harness.source, Layer::Global);
        assert_eq!(global_only.model.value, "anthropic/global-model");
        assert_eq!(global_only.model.source, Layer::Global);
        assert_eq!(global_only.harness_command.value, vec!["pi", "--global"]);
        assert_eq!(global_only.harness_command.source, Layer::Global);
    }

    #[test]
    fn resolve_runtime_falls_back_to_the_default_harness_command_when_unset() {
        let global = GlobalConfig {
            workspace: Some("/workspace".to_owned()),
            ..Default::default()
        };
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        let effective =
            resolve_runtime(&global, Some(&spec), &RuntimeOverrides::default()).unwrap();
        assert_eq!(
            effective.harness_command.value,
            vec!["pi".to_owned(), "--no-session".to_owned()]
        );
        assert_eq!(effective.harness_command.source, Layer::Default);
    }

    #[test]
    fn resolve_runtime_reports_a_clear_error_naming_the_missing_key_and_layers() {
        let global = GlobalConfig::default();
        let error = resolve_runtime(&global, None, &RuntimeOverrides::default()).unwrap_err();
        assert!(error.contains('`'), "{error}");
        assert!(error.contains("cli") && error.contains("spec") && error.contains("global"));
        // The first key checked (workspace) is the one that's reported missing.
        assert!(error.contains("workspace"), "{error}");
    }

    #[test]
    fn create_run_resolves_from_global_config_alone_with_no_spec() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let workspace = temp.join("workspace");
        fs::create_dir_all(workspace.join("dispatches")).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        fs::write(
            store.config_path(),
            format!(
                "workspace = '{}'\nimage = 'example'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
                workspace.display()
            ),
        )
        .unwrap();

        let (record, spec) = store
            .create_run(RuntimeOverrides::default(), None, vec![])
            .unwrap();
        assert!(record.spec.is_none(), "no --spec was given");
        assert_eq!(record.effective.image.value, "example");
        assert_eq!(record.effective.image.source, Layer::Global);
        assert_eq!(record.effective.harness.source, Layer::Global);
        assert_eq!(record.effective.model.source, Layer::Global);
        assert!(matches!(spec.sandbox.network, Network::None));
        assert!(!directory_has_spec_toml(&store, &record.id));

        let reloaded = store.read_run(&record.id).unwrap();
        assert_eq!(reloaded.effective.image.value, "example");
        fs::remove_dir_all(temp).unwrap();
    }

    fn directory_has_spec_toml(store: &Store, run_id: &str) -> bool {
        store.run_path(run_id).unwrap().join("spec.toml").is_file()
    }

    #[test]
    fn create_run_rejects_a_missing_required_key_naming_it() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let overrides = RuntimeOverrides {
            workspace: Some(workspace.display().to_string()),
            ..Default::default()
        };
        let error = store.create_run(overrides, None, vec![]).unwrap_err();
        assert!(error.contains("image"), "{error}");
    }

    #[test]
    fn resume_uses_the_frozen_effective_values_and_never_re_merges() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (mut record, _) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        assert_eq!(record.effective.model.value, "anthropic/claude-opus-4-6");

        // Simulate the global config changing after the run was created; `resume` must not
        // pick up the new value, since it never re-merges the layers.
        fs::write(
            store.config_path(),
            "[harness]\nmodel = 'anthropic/a-different-model'\n",
        )
        .unwrap();

        let directory = store.run_path(&record.id).unwrap();
        record.state = RunState::Stopped;
        record.image = Some("image-id".to_owned());
        record.image_profile = Some(SpecIdentity {
            name: "example".to_owned(),
            sha256: "deadbeef".to_owned(),
        });
        save_record(&directory, &record).unwrap();
        write_new(&directory.join("resolved.json"), b"{}").unwrap();

        let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  run) echo 'fake-container'; exit 0 ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
        let _docker = FakeDocker::install(script, &temp.join("bin"));

        let resumed = store.resume(&record.id).unwrap();
        assert_eq!(resumed.effective.model.value, "anthropic/claude-opus-4-6");
        fs::remove_dir_all(temp).unwrap();
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
        assert!(
            !arguments.iter().any(|argument| argument == "--no-session"),
            "Gardr must drop --no-session for the pi adapter so a transcript is written"
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--session", "/gardr-transcript/session.jsonl"])
        );
        assert!(
            arguments
                .iter()
                .any(|argument| argument.contains("target=/gardr-transcript"))
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn pi_dispatch_args_reject_session_management_flags() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        assert!(
            validate_dispatch_harness_args(&spec, &["--session".to_owned(), "x".to_owned()])
                .unwrap_err()
                .contains("session")
        );
        assert!(validate_dispatch_harness_args(&spec, &["--continue".to_owned()]).is_err());
        assert!(validate_dispatch_harness_args(&spec, &["-r".to_owned()]).is_err());
        assert!(validate_dispatch_harness_args(&spec, &["--no-session".to_owned()]).is_ok());
    }

    #[test]
    fn pi_name_flag_does_not_conflict_with_the_injected_session_flag() {
        // --name (session display name) doesn't conflict with Gardr's injected --session, so
        // specs and dispatch args using it must keep validating (no migration required).
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--name', 'my-session']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        assert!(validate_spec(&spec).is_ok());
        assert!(
            validate_dispatch_harness_args(&spec, &["--name".to_owned(), "x".to_owned()]).is_ok()
        );
        assert!(validate_dispatch_harness_args(&spec, &["-n".to_owned(), "x".to_owned()]).is_ok());
    }

    #[test]
    fn pi_command_reserves_session_management_flags() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--fork', 'x']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
        assert!(
            validate_spec(&spec)
                .unwrap_err()
                .contains("dispatch or model-selection")
        );
    }

    #[test]
    fn transcript_directory_is_created_for_pi_runs_and_recorded_in_state() {
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
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(workspace.join("dispatches")).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (record, _spec) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        let transcript_path = record.transcript_path.clone().unwrap();
        assert!(transcript_path.contains(&record.id));
        assert!(Path::new(&transcript_path).parent().unwrap().is_dir());
        assert!(record.stdout_path.ends_with("stdout.log"));
        assert!(record.stderr_path.ends_with("stderr.log"));
        let reloaded = store.read_run(&record.id).unwrap();
        assert_eq!(reloaded.transcript_path, record.transcript_path);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn transcript_usage_is_summed_from_session_jsonl() {
        let temp = temporary_directory();
        let transcript = temp.join("session.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"s\"}\n",
                "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":1,\"cacheWrite\":2,\"totalTokens\":18,\"cost\":{\"input\":0.01,\"output\":0.02,\"cacheRead\":0.0,\"cacheWrite\":0.0,\"total\":0.03}}}}\n",
                "{\"type\":\"compaction\",\"id\":\"b\",\"parentId\":\"a\",\"summary\":\"s\",\"tokensBefore\":1,\"usage\":{\"input\":1,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0.001}}}\n",
            ),
        )
        .unwrap();
        let (usage, skipped) = read_pi_transcript_usage(&transcript).unwrap();
        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 6);
        assert!((usage.total_cost - 0.031).abs() < 1e-9);
        assert_eq!(skipped, 0);
        let (missing_usage, missing_skipped) =
            read_pi_transcript_usage(&temp.join("missing.jsonl")).unwrap();
        assert!(missing_usage.is_none());
        assert_eq!(missing_skipped, 0);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn transcript_usage_tolerates_a_torn_trailing_line_but_flags_other_unparsable_lines() {
        let temp = temporary_directory();
        let transcript = temp.join("session.jsonl");
        fs::write(
            &transcript,
            concat!(
                "not json at all\n",
                "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cost\":{\"total\":0.05}}}}\n",
                "{\"type\":\"message\",\"id\":\"b\",\"parentId\":\"a\",\"message\":{\"role\":\"assistant\",\"con", // torn trailing write
            ),
        )
        .unwrap();
        let (usage, skipped) = read_pi_transcript_usage(&transcript).unwrap();
        let usage = usage.expect("usage collected despite bad lines");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(
            skipped, 1,
            "the leading unparsable line is flagged; the torn trailing line is tolerated"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn transcript_usage_surfaces_io_errors_distinct_from_no_transcript_yet() {
        let temp = temporary_directory();
        // A directory in place of the transcript file yields a real IO error ("Is a directory")
        // distinct from `NotFound`; this test runs as root, so permission bits alone wouldn't
        // reliably block a read.
        let transcript = temp.join("session.jsonl");
        fs::create_dir(&transcript).unwrap();
        let result = read_pi_transcript_usage(&transcript);
        assert!(
            result.is_err(),
            "an unreadable transcript must surface an error, not be conflated with 'no spend yet'"
        );
        let (missing_usage, missing_skipped) =
            read_pi_transcript_usage(&temp.join("missing.jsonl")).unwrap();
        assert!(
            missing_usage.is_none(),
            "a missing transcript is 'no spend yet', not an error"
        );
        assert_eq!(missing_skipped, 0);
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
    fn cleanup_captures_stdout_and_stderr_before_docker_rm_and_stays_idempotent() {
        let temp = temporary_directory();
        let order_file = temp.join("order.log");
        let script = format!(
            "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) echo \"logs $2\" >> '{order}'; printf 'OUT-CONTENT'; printf 'ERR-CONTENT' >&2 ;;\n  rm) echo \"rm $2\" >> '{order}' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n",
            order = order_file.display()
        );
        let _docker = FakeDocker::install(&script, &temp.join("bin"));

        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (mut record, _) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        let directory = store.run_path(&record.id).unwrap();
        record.state = RunState::Running;
        record.container = Some("fake-container".to_owned());
        save_record(&directory, &record).unwrap();

        store.cleanup(&record.id).unwrap();

        assert!(directory.join("cleanup.complete").exists());
        assert_eq!(
            fs::read_to_string(directory.join("stdout.log")).unwrap(),
            "OUT-CONTENT"
        );
        assert_eq!(
            fs::read_to_string(directory.join("stderr.log")).unwrap(),
            "ERR-CONTENT"
        );
        let order = fs::read_to_string(&order_file).unwrap();
        let logs_at = order.find("logs fake-container").expect("logs invoked");
        let rm_at = order.find("rm fake-container").expect("rm invoked");
        assert!(
            logs_at < rm_at,
            "docker logs must run before docker rm: {order}"
        );

        // Idempotent: cleanup again must not re-invoke docker (order file unchanged).
        store.cleanup(&record.id).unwrap();
        assert_eq!(fs::read_to_string(&order_file).unwrap(), order);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn cleanup_removes_the_container_and_reaches_a_terminal_state_when_log_capture_fails() {
        // An ordinary `docker logs` failure (unreadable log driver, transient daemon hiccup,
        // permissions) must not leak the container: cleanup is best-effort about log capture and
        // still proceeds to `docker rm` and `cleanup.complete`.
        let temp = temporary_directory();
        let order_file = temp.join("order.log");
        let script = format!(
            "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) echo \"logs $2\" >> '{order}'; echo 'error during connect: transient daemon hiccup' >&2; exit 1 ;;\n  rm) echo \"rm $2\" >> '{order}' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n",
            order = order_file.display()
        );
        let _docker = FakeDocker::install(&script, &temp.join("bin"));

        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (mut record, _) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        let directory = store.run_path(&record.id).unwrap();
        record.state = RunState::Running;
        record.container = Some("fake-container".to_owned());
        save_record(&directory, &record).unwrap();

        store
            .cleanup(&record.id)
            .expect("cleanup must succeed and reach a terminal state despite log capture failure");

        assert!(directory.join("cleanup.complete").exists());
        let order = fs::read_to_string(&order_file).unwrap();
        assert!(
            order.contains("logs fake-container"),
            "docker logs was attempted"
        );
        assert!(
            order.contains("rm fake-container"),
            "docker rm still ran despite log failure"
        );
        let runner_log = fs::read_to_string(directory.join("runner.log")).unwrap();
        assert!(
            runner_log.contains("log capture failed"),
            "the log capture failure must be recorded: {runner_log}"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn observe_surfaces_cost_from_the_pi_transcript() {
        let temp = temporary_directory();
        let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'true 0' ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
        let _docker = FakeDocker::install(script, &temp.join("bin"));

        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (mut record, _) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        let directory = store.run_path(&record.id).unwrap();
        record.state = RunState::Running;
        record.container = Some("fake-container".to_owned());
        save_record(&directory, &record).unwrap();
        fs::write(
            record.transcript_path.as_deref().unwrap(),
            "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0.05}}}}\n",
        )
        .unwrap();

        let observed = store.observe(&record.id).unwrap();
        let usage = observed.usage.expect("usage should be surfaced");
        assert!((usage.total_cost - 0.05).abs() < 1e-9);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
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

    #[test]
    fn credentials_can_be_registered_listed_and_removed_without_exposing_values() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        store.set_credential("gh-pat", b"super-secret").unwrap();
        assert_eq!(store.list_credentials().unwrap(), ["gh-pat".to_owned()]);
        assert_eq!(
            fs::read(store.credential_path("gh-pat").unwrap()).unwrap(),
            b"super-secret"
        );
        // Re-registering (upsert) rotates the value rather than failing like spec/image `add`.
        store.set_credential("gh-pat", b"rotated-secret").unwrap();
        assert_eq!(
            fs::read(store.credential_path("gh-pat").unwrap()).unwrap(),
            b"rotated-secret"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.credentials_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(store.credential_path("gh-pat").unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        store.remove_credential("gh-pat").unwrap();
        assert_eq!(store.list_credentials().unwrap(), Vec::<String>::new());
        assert!(store.remove_credential("gh-pat").is_err());
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn credential_environment_entries_support_plain_string_and_mapped_table_forms() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN', {name = 'GH_TOKEN2', from = 'gh-pat'}, {name = 'GH_TOKEN3'}]\n").unwrap();
        validate_spec(&spec).unwrap();
        let refs = &spec.credentials.environment;
        assert_eq!(refs[0].env_name(), "GH_TOKEN");
        assert_eq!(refs[0].registry_key(), "GH_TOKEN");
        assert_eq!(refs[1].env_name(), "GH_TOKEN2");
        assert_eq!(refs[1].registry_key(), "gh-pat");
        assert_eq!(refs[2].env_name(), "GH_TOKEN3");
        assert_eq!(refs[2].registry_key(), "GH_TOKEN3");
    }

    #[test]
    fn duplicate_credential_environment_names_are_rejected_at_validation() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = [{name = 'GH_TOKEN', from = 'gh-pat-a'}, {name = 'GH_TOKEN', from = 'gh-pat-b'}]\n").unwrap();
        assert!(
            validate_spec(&spec)
                .unwrap_err()
                .contains("duplicate credential environment name")
        );
    }

    #[test]
    fn docker_arguments_resolves_credentials_from_the_store_via_from_and_env_file() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = [{name = 'GH_TOKEN', from = 'gh-pat'}]\n").unwrap();
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        store.set_credential("gh-pat", b"super-secret\n").unwrap();
        let arguments = docker_arguments(
            &store,
            &spec,
            &temp,
            &temp,
            "example",
            "run-test",
            "image-id",
            &[],
        )
        .unwrap();
        let env_file_index = arguments
            .iter()
            .position(|argument| argument == "--env-file")
            .expect("--env-file must be present");
        let env_file = PathBuf::from(&arguments[env_file_index + 1]);
        let contents = fs::read_to_string(&env_file).unwrap();
        assert_eq!(contents, "GH_TOKEN=super-secret\n");
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("super-secret")),
            "the secret value must not appear directly on the docker argv: {arguments:?}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&env_file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn docker_arguments_fails_clearly_for_an_unregistered_credential() {
        let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN']\n").unwrap();
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let error = docker_arguments(
            &store,
            &spec,
            &temp,
            &temp,
            "example",
            "run-test",
            "image-id",
            &[],
        )
        .unwrap_err();
        assert!(
            error.contains("required credential is not registered: GH_TOKEN"),
            "{error}"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn credential_set_rejects_multi_line_and_comment_looking_values() {
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        let error = store
            .set_credential(
                "ssh-key",
                b"-----BEGIN KEY-----\nsecret-bytes\n-----END KEY-----\n",
            )
            .unwrap_err();
        assert!(error.contains("single line"), "{error}");
        assert!(!store.credential_path("ssh-key").unwrap().exists());

        let error = store
            .set_credential("comment", b"#not-a-comment")
            .unwrap_err();
        assert!(error.contains('#'), "{error}");

        // A single trailing newline (a common artifact of files written by editors) is tolerated.
        store.set_credential("token", b"plain-value\n").unwrap();
        assert_eq!(
            fs::read(store.credential_path("token").unwrap()).unwrap(),
            b"plain-value\n"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn atomic_write_does_not_clobber_a_sibling_credential_whose_name_is_a_prefix() {
        // `validate_name` allows `.` in a credential name; registering `a.b` must not go through a
        // temp path (like the old `<name>.new`) that collides with a distinct, already-registered
        // credential `a`.
        let temp = temporary_directory();
        let store = Store::open(temp.join("store"));
        store.set_credential("a", b"a-secret").unwrap();
        store.set_credential("a.b", b"a-b-secret").unwrap();
        assert_eq!(
            fs::read(store.credential_path("a").unwrap()).unwrap(),
            b"a-secret",
            "registering a.b must not clobber the unrelated credential a"
        );
        assert_eq!(
            fs::read(store.credential_path("a.b").unwrap()).unwrap(),
            b"a-b-secret"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn credentials_env_does_not_survive_cleanup() {
        let temp = temporary_directory();
        let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) exit 0 ;;\n  rm) exit 0 ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
        let _docker = FakeDocker::install(script, &temp.join("bin"));

        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(&source, spec()).unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
        let (mut record, _) = store
            .create_run(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        let directory = store.run_path(&record.id).unwrap();
        // Simulate a run that resolved credentials into the plaintext env-file, as `launch` would
        // before invoking `docker run` (independent of exercising the credential-resolution path).
        let credentials_env = directory.join("credentials.env");
        atomic_write(&credentials_env, b"GH_TOKEN=super-secret\n").unwrap();
        assert!(credentials_env.exists());
        record.state = RunState::Stopped;
        record.container = Some("fake-container".to_owned());
        save_record(&directory, &record).unwrap();

        store.cleanup(&record.id).unwrap();

        assert!(
            !credentials_env.exists(),
            "cleanup must remove the plaintext resolved-credentials env-file"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn launch_removes_credentials_env_after_docker_run_returns() {
        let temp = temporary_directory();
        let script = "#!/bin/sh\nset -eu\nif [ \"$1\" = 'image' ] && [ \"$2\" = 'inspect' ]; then\n  if [ \"$3\" = '--format' ]; then\n    echo 'sha256:fakeid'\n  fi\n  exit 0\nfi\nif [ \"$1\" = 'run' ]; then\n  echo 'fake-container'\n  exit 0\nfi\necho \"unexpected docker command: $*\" >&2\nexit 1\n";
        let _docker = FakeDocker::install(script, &temp.join("bin"));

        let store = Store::open(temp.join("store"));
        let source = temp.join("spec.toml");
        fs::write(
            &source,
            b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN']\n",
        )
        .unwrap();
        let image_source = temp.join("example.toml");
        fs::write(
            &image_source,
            b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
        )
        .unwrap();
        store.add_image("example", &image_source).unwrap();
        store.add_spec("build", &source).unwrap();
        store.set_credential("GH_TOKEN", b"super-secret").unwrap();
        let workspace = temp.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(store.pi_agent_path()).unwrap();
        fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
        set_private_directory(&store.pi_agent_path()).unwrap();
        set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();

        let record = store
            .start(overrides_for(&workspace), Some("build"), vec![])
            .unwrap();
        assert!(matches!(record.state, RunState::Running));
        let directory = store.run_path(&record.id).unwrap();
        assert!(
            !directory.join("credentials.env").exists(),
            "credentials.env must not survive past docker run returning"
        );
        fs::remove_dir_all(temp).unwrap();
    }
}

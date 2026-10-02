use crate::*;

/// Validates a spec's own unlayered runtime dependencies before resolution.
pub(crate) fn validate_runtime_spec(store: &Store, spec: &Spec) -> Result<()> {
    if matches!(spec.harness.adapter, Some(Adapter::Pi)) {
        validate_pi_agent(store)?;
    }
    for mount in &spec.mounts {
        approved_child(&store.mounts_path(), &mount.name)?;
    }
    Ok(())
}

/// An offline resolved run cannot install tools.
pub(crate) fn validate_resolved_network(spec: &Spec) -> Result<()> {
    if !spec.tools.install.is_empty() && matches!(resolved_network(spec), Network::None) {
        return Err("tools require the resolved sandbox network to be 'bridge'".to_owned());
    }
    Ok(())
}

/// Validates the complete resolved spec before a run is prepared or resumed.
pub(crate) fn validate_resolved_spec(store: &Store, spec: &Spec) -> Result<()> {
    validate_resolved_harness(&spec.harness)?;
    validate_resolved_network(spec)?;
    validate_runtime_spec(store, spec)?;
    validate_image_requirements(store, spec)?;
    Ok(())
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self};
use std::io;
use std::path::Path;

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
    #[serde(default)]
    pub sandbox: Sandbox,
    #[serde(default)]
    pub harness: Harness,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub credentials: Credentials,
    #[serde(default)]
    pub tools: Tooling,
    /// Global-only command prefix and container mounts copied in after parsing. `serde(skip)`
    /// keeps both unavailable to legacy spec files while allowing the established execution path
    /// to carry the frozen global runtime policy.
    #[serde(skip)]
    pub startup_command: Vec<String>,
    #[serde(skip)]
    pub runtime_mounts: Vec<RuntimeMount>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    /// A spec may only ever *narrow* the globally configured network mode to `none`; it cannot
    /// request `bridge` itself (`bridge` is decided solely by the global config). Absent, the
    /// global config's (or `run start --network none`'s) resolution applies unchanged.
    #[serde(default)]
    pub network: Option<Network>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    None,
    Bridge,
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
pub struct Tooling {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub install: Vec<Tool>,
}
/// Installer domains are no longer declared per-tool: an installer runs with the same single
/// global `[firewall] allow` egress as the harness itself (see `gardr docs`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub check: String,
    pub install: Vec<Vec<String>>,
}

pub fn parse_spec(content: &[u8]) -> Result<Spec> {
    let text =
        std::str::from_utf8(content).map_err(|error| format!("run spec is not UTF-8: {error}"))?;
    let raw: toml::Value =
        toml::from_str(text).map_err(|error| format!("invalid run spec: {error}"))?;
    reject_removed_spec_keys(&raw)?;
    toml::from_str(text).map_err(|error| format!("invalid run spec: {error}"))
}
/// Firewall policy moved to the global config only: a spec's `[firewall]` and
/// `[[tools.install]].allow` are removed keys, not fields silently dropped by
/// `deny_unknown_fields`'s generic message. Named explicitly so a stored spec written before this
/// change fails clearly instead of with a bare "unknown field" error.
fn reject_removed_spec_keys(raw: &toml::Value) -> Result<()> {
    if raw.get("firewall").is_some() {
        return Err(
            "spec `[firewall]` is no longer supported: firewall policy is configured only in \
             the global config's `[firewall] allow` now; remove `[firewall]` from this spec (see \
             `gardr docs`)"
                .to_owned(),
        );
    }
    let declares_install_allow = raw
        .get("tools")
        .and_then(|tools| tools.get("install"))
        .and_then(|install| install.as_array())
        .is_some_and(|installs| installs.iter().any(|tool| tool.get("allow").is_some()));
    if declares_install_allow {
        return Err(
            "spec `[[tools.install]].allow` is no longer supported: installer egress comes only \
             from the global config's `[firewall] allow` now; remove `allow` from each \
             `[[tools.install]]` entry (see `gardr docs`)"
                .to_owned(),
        );
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
        if matches!(
            mount.target.as_str(),
            "/workspace" | "/repo" | "/gardr-context" | "/gardr-input"
        ) {
            return Err(format!("{} is reserved by Gardr", mount.target));
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
    if matches!(spec.sandbox.network, Some(Network::Bridge)) {
        return Err(
            "a spec's sandbox.network may only override the global config to 'none'; 'bridge' is \
             decided solely by the global config's network default"
                .to_owned(),
        );
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
    Ok(())
}

impl Store {
    pub fn validate_runtime_spec(&self, spec: &Spec) -> Result<()> {
        validate_runtime_spec(self, spec)
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
}

impl Store {
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
}

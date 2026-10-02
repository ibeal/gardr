use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self};
use std::io;
use std::path::Path;

/// The optional global config at `<root>/config.toml`. Every key is a default consulted only when
/// the spec and CLI layers leave it unset; a missing file resolves to every field `None`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// Default workspace path, overridden by a spec's `workspace` and by `run start --workspace`.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Removed compatibility field. A parsed config that declares it receives a migration error.
    #[serde(default, skip)]
    pub spec: Option<String>,
    /// Default image profile name, overridden by a spec's `image.name` and `run start --image`.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub harness: Option<GlobalHarness>,
    /// The default sandbox network mode for every run. A spec's `sandbox.network` and
    /// `run start --network` may only narrow this to `none`; neither can force `bridge`. Unset
    /// everywhere, a run resolves to `none` (the safe default): bridge egress is never granted
    /// by omission.
    #[serde(default)]
    pub network: Option<Network>,
    /// The single global egress allowlist applied to every bridge run; there is no per-spec
    /// `[firewall]` and no separate hardcoded minimum or installer-only list. A bridge run whose
    /// resolved allowlist is empty fails explicitly at `run start`.
    #[serde(default)]
    pub firewall: Option<GlobalFirewall>,
    /// Credential references injected into every run. Values still come from Gardr's private
    /// credential registry; only names and optional aliases are configuration.
    #[serde(default)]
    pub credentials: Credentials,
    /// Optional command placed between Gardr's firewall bootstrap and the harness command. This is
    /// useful for image-independent wrappers such as shell or environment setup commands.
    #[serde(default)]
    pub startup_command: Vec<String>,
    /// Trusted, global container mounts. They are deliberately not accepted from legacy specs.
    #[serde(default)]
    pub mounts: Vec<RuntimeMount>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMount {
    #[serde(rename = "type")]
    pub kind: RuntimeMountType,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMountType {
    Bind,
    Volume,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalFirewall {
    #[serde(default)]
    pub allow: Vec<String>,
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
    let text = std::str::from_utf8(content)
        .map_err(|error| format!("global config is not UTF-8: {error}"))?;
    let raw: toml::Value =
        toml::from_str(text).map_err(|error| format!("invalid global config: {error}"))?;
    if raw.get("spec").is_some() {
        return Err("global config key `spec` has been removed; move runtime policy into config.toml and remove the key".to_owned());
    }
    let global: GlobalConfig =
        toml::from_str(text).map_err(|error| format!("invalid global config: {error}"))?;
    for domain in global
        .firewall
        .iter()
        .flat_map(|firewall| firewall.allow.iter())
    {
        validate_domain(domain)?;
    }
    let mut credential_names = BTreeSet::new();
    for reference in &global.credentials.environment {
        validate_environment_reference(reference.env_name())?;
        validate_name("credential registry key", reference.registry_key())?;
        if !credential_names.insert(reference.env_name()) {
            return Err(format!(
                "duplicate credential environment name: {}",
                reference.env_name()
            ));
        }
    }
    if !global.startup_command.is_empty() {
        validate_command("startup command", &global.startup_command)?;
    }
    let mut mount_targets = BTreeSet::new();
    for mount in &global.mounts {
        validate_container_path("mount target", &mount.target)?;
        if matches!(
            mount.target.as_str(),
            "/workspace"
                | "/repo"
                | "/gardr"
                | "/gardr-context"
                | "/gardr-input"
                | "/pi-agent"
                | "/gardr-transcript"
                | "/usr/local/bin/init-firewall.sh"
        ) {
            return Err(format!(
                "mount target is reserved by Gardr: {}",
                mount.target
            ));
        }
        if !mount_targets.insert(&mount.target) {
            return Err(format!("duplicate mount target: {}", mount.target));
        }
        match mount.kind {
            RuntimeMountType::Volume => validate_name("Docker volume name", &mount.source)?,
            RuntimeMountType::Bind => {
                let source = Path::new(&mount.source);
                if !source.is_absolute() {
                    return Err(format!(
                        "bind mount source must be absolute: {}",
                        mount.source
                    ));
                }
                validate_docker_path("bind mount source", source)?;
            }
        }
    }
    Ok(global)
}
impl Store {
    /// Reads the optional global config at `<root>/config.toml`. A missing file is not an error;
    /// it resolves to an all-`None` `GlobalConfig`.
    pub fn read_global_config(&self) -> Result<GlobalConfig> {
        match fs::read(self.config_path()) {
            Ok(content) => parse_global_config(&content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(GlobalConfig::default()),
            Err(error) => Err(io_error(error)),
        }
    }
}

pub(crate) mod legacy;
#[cfg(test)]
mod legacy_tests;
pub(crate) mod resolve;
#[cfg(test)]
mod tests;

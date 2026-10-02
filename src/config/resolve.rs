use crate::*;
use serde::{Deserialize, Serialize};

/// The `run start` command-line overrides for the layered runtime keys. Each, if present, wins
/// over the spec and global config layers for that key.
#[derive(Clone, Debug, Default)]
pub struct RuntimeOverrides {
    pub workspace: Option<String>,
    pub image: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    /// `--network none`: forces the resolved network to `none` regardless of the spec or global
    /// config. There is no CLI way to force `bridge`; that is the global config's decision alone.
    pub network_none: bool,
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
    pub network: Resolved<Network>,
    /// The global config's `[firewall] allow` list, frozen exactly as read at `run start`. This
    /// is the single egress allowlist; `resume` reuses this value verbatim and never re-reads the
    /// global config.
    pub firewall_allow: Resolved<Vec<String>>,
    /// Global credential references frozen with the run. Values are resolved from the private
    /// registry only when a container is launched. `None` is reserved for records created before
    /// this field existed, whose frozen legacy spec remains authoritative.
    #[serde(default)]
    pub credentials: Option<Credentials>,
    /// Global-only generic container policy. `None` preserves legacy records created before these
    /// fields existed; new records always freeze `Some`, including an empty value.
    #[serde(default)]
    pub startup_command: Option<Vec<String>>,
    #[serde(default)]
    pub mounts: Option<Vec<RuntimeMount>>,
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
/// Merges the sandbox network mode: `run start --network none` or a spec's `sandbox.network =
/// "none"` force the network off; neither can force `bridge`, which is decided solely by the
/// global config. Falls back to `none` (the safe default) when nothing sets it.
pub(crate) fn merge_network(
    cli_none: bool,
    spec: Option<Network>,
    global: Option<Network>,
) -> (Network, Layer) {
    if cli_none {
        return (Network::None, Layer::Cli);
    }
    if let Some(Network::None) = spec {
        return (Network::None, Layer::Spec);
    }
    if let Some(network) = global {
        return (network, Layer::Global);
    }
    (Network::None, Layer::Default)
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
pub(crate) fn resolve_runtime(
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
    let (network, network_source) = merge_network(
        overrides.network_none,
        spec.and_then(|spec| spec.sandbox.network),
        global.network,
    );
    let firewall_allow = global
        .firewall
        .as_ref()
        .map(|firewall| firewall.allow.clone())
        .unwrap_or_default();
    if matches!(network, Network::Bridge) && firewall_allow.is_empty() {
        return Err(
            "missing required configuration: a bridge run requires a non-empty global config \
             `[firewall] allow` list; gardr ships no hardcoded minimum allowlist"
                .to_owned(),
        );
    }
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
        network: Resolved {
            value: network,
            source: network_source,
        },
        firewall_allow: Resolved {
            value: firewall_allow,
            source: Layer::Global,
        },
        credentials: Some(
            spec.map(|spec| spec.credentials.clone())
                .filter(|credentials| !credentials.environment.is_empty())
                .unwrap_or_else(|| global.credentials.clone()),
        ),
        startup_command: Some(global.startup_command.clone()),
        mounts: Some(global.mounts.clone()),
    })
}
/// The neutral baseline used when a run has no spec at all: no image/harness/model of its own (all
/// filled in by `apply_effective_runtime`), no mounts/credentials/tools, and no network of its own
/// (also filled in by `apply_effective_runtime` from the merged, global-only network mode).
pub(crate) fn implicit_spec() -> Spec {
    Spec {
        version: 1,
        workspace: None,
        image: Image::default(),
        sandbox: Sandbox { network: None },
        harness: Harness::default(),
        mounts: Vec::new(),
        credentials: Credentials::default(),
        tools: Tooling::default(),
        startup_command: Vec::new(),
        runtime_mounts: Vec::new(),
    }
}
/// Overwrites a baseline spec's layered fields (`image.name`, `harness.adapter`,
/// `harness.command`, `harness.model`, `sandbox.network`) with the merged effective values,
/// leaving every unlayered field (mounts, credentials, tools) exactly as the baseline declared it.
pub(crate) fn apply_effective_runtime(mut spec: Spec, effective: &EffectiveRuntime) -> Spec {
    spec.image.name = Some(effective.image.value.clone());
    spec.harness.adapter = Some(effective.harness.value);
    spec.harness.command = effective.harness_command.value.clone();
    spec.harness.model = Some(effective.model.value.clone());
    spec.sandbox.network = Some(effective.network.value);
    if let Some(credentials) = &effective.credentials {
        spec.credentials = credentials.clone();
    }
    if let Some(startup_command) = &effective.startup_command {
        spec.startup_command = startup_command.clone();
    }
    if let Some(mounts) = &effective.mounts {
        spec.runtime_mounts = mounts.clone();
    }
    spec
}
/// The resolved network mode of a spec that has already been merged via `apply_effective_runtime`.
pub(crate) fn resolved_network(spec: &Spec) -> Network {
    spec.sandbox
        .network
        .expect("network is resolved by apply_effective_runtime before use")
}

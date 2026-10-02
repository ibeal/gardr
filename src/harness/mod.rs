use crate::*;
use serde::{Deserialize, Serialize};

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
/// Default `harness.command` per adapter, used when neither a spec nor the global config sets
/// one.
pub(crate) fn default_harness_command(adapter: Adapter) -> Vec<String> {
    match adapter {
        Adapter::Pi => vec!["pi".to_owned()],
        Adapter::ClaudeCode => vec!["claude".to_owned()],
    }
}

/// Validates that the merged `harness` (adapter, command, model) is complete and internally
/// consistent: the resolved fields formerly checked only at `spec add` — supported adapter,
/// command shape, provider-qualified model — now checked against the merged result.
pub(crate) fn validate_resolved_harness(harness: &Harness) -> Result<()> {
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

pub(crate) mod pi;

pub(crate) mod transcript;

#[cfg(test)]
mod tests;

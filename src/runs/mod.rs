use crate::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct StartRequest {
    pub repo: Option<PathBuf>,
    pub url: Option<String>,
    pub thread: Option<String>,
    pub ask_file: Option<PathBuf>,
    pub current_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRecord {
    pub kind: SourceKind,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
}
impl SourceRecord {
    fn directory(path: PathBuf) -> Self {
        Self {
            kind: SourceKind::Directory,
            path: path.display().to_string(),
            url: None,
            workspace_id: None,
            workspace_path: None,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Directory,
    Url,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ThreadRecord {
    source: SourceRecord,
}
#[derive(Serialize)]
pub(crate) struct OneShotMetadata<'a> {
    version: u32,
    id: &'a str,
    source: &'a SourceRecord,
    thread: Option<&'a str>,
    input_path: Option<&'a str>,
    effective: &'a EffectiveRuntime,
    created_at: u64,
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
    /// One-based process attempt within this run. Legacy records default to their first attempt.
    #[serde(default = "default_attempt")]
    pub attempt: u32,
    /// Present for one-shot runs. Legacy resumable records omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_path: Option<String>,
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

pub(crate) mod source;

pub(crate) mod prepare;

pub(crate) mod lifecycle;

pub(crate) mod persistence;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lifecycle_tests;

use crate::*;
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);
use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) fn save_record(directory: &Path, record: &RunRecord) -> Result<()> {
    atomic_write(
        &directory.join("state.json"),
        &serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?,
    )
}
pub(crate) fn append_log(directory: &Path, message: &str) -> Result<()> {
    use std::io::Write;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("runner.log"))
        .map_err(io_error)?;
    writeln!(file, "{} {message}", now()).map_err(io_error)
}
pub(crate) fn new_run_id() -> String {
    format!(
        "run-{}-{}-{}",
        now(),
        std::process::id(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
pub(crate) fn new_workspace_id() -> String {
    format!(
        "workspace-{}-{}-{}",
        now(),
        std::process::id(),
        RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
pub(crate) fn ensure_base_context(store: &Store) -> Result<()> {
    let path = store.base_context_path();
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path.parent().expect("base context has a parent")).map_err(io_error)?;
    write_new(
        &path,
        b"# Gardr agent context\n\nWork in /repo. Each run is a fresh session. If /gardr-context/CONTINUITY.md exists, read it before working and keep it concise: decisions, current state, verification, blockers, and next work only. Never copy the conversation transcript into continuity.\n",
    )
}
pub(crate) fn prepare_one_shot_resume(directory: &Path, record: &mut RunRecord) -> Result<()> {
    if let Some(container) = record.container.as_deref() {
        match docker_status(container)? {
            Some((true, _)) => return Err(format!("run {} is still running", record.id)),
            Some((false, _)) => capture_run_logs(record)?,
            None => {}
        }
        let output = Command::new("docker")
            .args(["rm", container])
            .output()
            .map_err(io_error)?;
        if !output.status.success()
            && !String::from_utf8_lossy(&output.stderr).contains("No such container")
        {
            return Err(command_error("docker rm", &output));
        }
    }
    record.attempt = record.attempt.max(1) + 1;
    record.stdout_path = directory
        .join(format!("stdout.attempt-{}.log", record.attempt))
        .display()
        .to_string();
    record.stderr_path = directory
        .join(format!("stderr.attempt-{}.log", record.attempt))
        .display()
        .to_string();
    record.state = RunState::Prepared;
    record.container = None;
    record.exit_status = None;
    record.failure = None;
    record.updated_at = now();
    save_record(directory, record)
}
pub(crate) fn default_attempt() -> u32 {
    1
}
pub(crate) fn command_error(name: &str, output: &std::process::Output) -> String {
    format!(
        "{name} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}
pub(crate) fn io_error(error: io::Error) -> String {
    error.to_string()
}
impl Store {
    pub fn read_run(&self, id: &str) -> Result<RunRecord> {
        let path = self.run_path(id)?.join("state.json");
        let bytes = fs::read(path).map_err(io_error)?;
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid run state: {error}"))
    }
}

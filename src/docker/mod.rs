use crate::*;
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

/// Captures a container's raw stdout/stderr via `docker logs`, streaming directly into the two
/// destination files (rather than buffering the whole output in memory) so capture is incremental
/// and doesn't risk OOM on a chatty run.
pub(crate) fn capture_container_logs(
    container: &str,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<()> {
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

pub(crate) fn docker_status(container: &str) -> Result<Option<(bool, Option<i32>)>> {
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
pub(crate) fn capture_run_logs(record: &RunRecord) -> Result<()> {
    let Some(container) = record.container.as_deref() else {
        return Ok(());
    };
    capture_container_logs(
        container,
        Path::new(&record.stdout_path),
        Path::new(&record.stderr_path),
    )
}

pub(crate) mod image;

pub(crate) mod arguments;

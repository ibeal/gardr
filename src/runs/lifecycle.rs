use crate::*;
use std::fs::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fail_run(directory: &Path, record: &mut RunRecord, error: String) -> Result<RunRecord> {
    record.state = RunState::Failed;
    record.failure = Some(error.clone());
    record.updated_at = now();
    append_log(directory, &error)?;
    save_record(directory, record)?;
    Err(error)
}

impl Store {
    pub fn resume(&self, id: &str) -> Result<RunRecord> {
        let mut record = self.read_run(id)?;
        let directory = self.run_path(id)?;
        if directory.join("cleanup.complete").exists() {
            return Err("cleaned runs are terminal and cannot be resumed".to_owned());
        }
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
                    save_record(&directory, &record)?;
                }
                Some((true, _)) => {
                    return Err(format!("run {id} is still running"));
                }
                None if record.source.is_some() => {
                    record.state = RunState::Failed;
                    record.failure = Some("container is no longer available".to_owned());
                    record.updated_at = now();
                    save_record(&directory, &record)?;
                }
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
        if record.source.is_some() && record.exit_status == Some(0) {
            return Err(format!(
                "run {id} completed successfully and is terminal; start a new run for a new ask"
            ));
        }
        // `resume` never re-merges global policy or accepts replacement source/input. It uses the
        // frozen effective values, original input.md, and existing Pi transcript. Legacy records
        // additionally reload their frozen spec for static unlayered fields.
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
        if record.source.is_some() {
            prepare_one_shot_resume(&directory, &mut record)?;
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
                    if let Err(error) = capture_run_logs(&record) {
                        append_log(&self.run_path(id)?, &format!("log capture failed: {error}"))?;
                    }
                    record.updated_at = now();
                    save_record(&self.run_path(id)?, &record)?;
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
}

impl Store {
    pub(crate) fn launch(
        &self,
        mut record: RunRecord,
        workspace: PathBuf,
        spec: Spec,
    ) -> Result<RunRecord> {
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
                    write_bootstrap(&directory, &spec, &record.effective.firewall_allow.value)?;
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

use super::*;
pub(super) fn run_command(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "start" => {
            let network = option(&mut args, "--network");
            let network_none = match network.as_deref() {
                None => false,
                Some("none") => true,
                Some(other) => {
                    return Err(format!(
                        "--network only accepts 'none' (bridge is decided by the global config): {other}"
                    ));
                }
            };
            if args.iter().any(|argument| argument == "--workspace") {
                return Err(
                    "--workspace has been removed; use --repo <path> (or --url <git-url>)"
                        .to_owned(),
                );
            }
            let request = StartRequest {
                repo: checked_option(&mut args, "--repo")?.map(PathBuf::from),
                url: checked_option(&mut args, "--url")?,
                thread: checked_option(&mut args, "--thread")?,
                ask_file: checked_option(&mut args, "--ask-file")?.map(PathBuf::from),
                current_dir: env::current_dir().map_err(|error| error.to_string())?,
            };
            let overrides = gardr::RuntimeOverrides {
                image: option(&mut args, "--image"),
                harness: option(&mut args, "--harness"),
                model: option(&mut args, "--model"),
                network_none,
                ..Default::default()
            };
            if option(&mut args, "--spec").is_some() {
                return Err("--spec has been removed; configure runtime policy in <root>/config.toml and use deliberate CLI overrides only".to_owned());
            }
            let harness_args = options(&mut args, "--harness-arg");
            reject_extra(&args)?;
            let interactive = is_interactive(&request, &harness_args);
            let record = store.start_one_shot(request, overrides, harness_args)?;
            if interactive {
                attach_interactive(&record)
            } else {
                print_json(&record)
            }
        }
        "observe" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.observe(&id)?)
        }
        "resume" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.resume(&id)?)
        }
        "stop" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.stop(&id)?)
        }
        "cleanup" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            store.cleanup(&id)?;
            print_json(&serde_json::json!({"id": id, "cleaned": true}))
        }
        "validate-workspace" => {
            let workspace = take(&mut args)?;
            reject_extra(&args)?;
            print_json(
                &serde_json::json!({"workspace": validate_workspace(PathBuf::from(workspace).as_path())?}),
            )
        }
        _ => Err(usage()),
    }
}

/// Returns `true` when the caller's terminal should be attached to the launched container
/// session instead of printing a machine-readable run record. Interactive mode is chosen when
/// neither `--ask-file` nor any `--harness-arg` was supplied, meaning the operator expects to
/// drive the session manually.
pub(super) fn is_interactive(request: &StartRequest, harness_args: &[String]) -> bool {
    request.ask_file.is_none() && harness_args.is_empty()
}

/// Attaches the caller's terminal to the running container via `docker attach`. The container
/// was started with `--interactive --tty`, so Docker allocates a TTY and `attach` connects
/// stdin/stdout/stderr. The default Docker detach sequence is **Ctrl-P, Ctrl-Q**, which
/// disconnects the client while leaving the container running.
///
/// A failed `docker attach` (spawn error or non-zero exit) is surfaced as a Gardr error.
pub(super) fn attach_interactive(record: &gardr::RunRecord) -> Result<(), String> {
    let container = record
        .container
        .as_deref()
        .ok_or_else(|| "interactive attach requires a container identifier".to_owned())?;
    let mut command = Command::new("docker");
    command.args(["attach", container]);
    if let Some((key, value)) = herdr_agent_hint(env::var_os("HERDR_ENV")) {
        command.env(key, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to spawn docker attach: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "docker attach exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ))
    }
}

/// Returns the `HERDR_AGENT` environment variable to set on the host-visible `docker attach`
/// child when Gardr itself is running under Herdr, so Herdr can register the interactive
/// sandbox as a Pi agent. Gardr never mutates its own process environment and never forwards
/// Herdr variables into the container; only this one variable is added to the detached
/// `docker attach` child, and only when `HERDR_ENV=1` is present on Gardr's own environment.
/// Autonomous runs never call this, so they remain undetected by Herdr.
pub(super) fn herdr_agent_hint(
    herdr_env: Option<OsString>,
) -> Option<(&'static str, &'static str)> {
    if herdr_env.as_deref() == Some(std::ffi::OsStr::new("1")) {
        Some(("HERDR_AGENT", "pi"))
    } else {
        None
    }
}

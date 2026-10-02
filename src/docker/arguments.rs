use crate::*;
use std::fs::{self};
use std::io;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) fn docker_arguments(
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
        "--interactive".to_owned(),
        "--tty".to_owned(),
        "--env".to_owned(),
        "TERM".to_owned(),
        "--env".to_owned(),
        "COLORTERM".to_owned(),
        "--name".to_owned(),
        format!("gardr-{spec_name}-{run_id}"),
        "--network".to_owned(),
        match resolved_network(spec) {
            Network::None => "none",
            Network::Bridge => "bridge",
        }
        .to_owned(),
        "--mount".to_owned(),
        format!("type=bind,source={},target=/repo", workspace.display()),
        "--workdir".to_owned(),
        "/repo".to_owned(),
        "--mount".to_owned(),
        format!(
            "type=bind,source={},target=/gardr-context/AGENTS.md,readonly",
            store.base_context_path().display()
        ),
    ];
    let continuity_marker = run_directory.join("continuity.path");
    if continuity_marker.is_file() {
        let continuity = fs::read_to_string(&continuity_marker).map_err(io_error)?;
        validate_docker_path("continuity path", Path::new(&continuity))?;
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target=/gardr-context/CONTINUITY.md",
                continuity
            ),
        ]);
    }
    let input = run_directory.join("input.md");
    if input.is_file() {
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target=/gardr-input/task.md,readonly",
                input.display()
            ),
        ]);
    }
    if matches!(resolved_network(spec), Network::Bridge) {
        args.extend([
            "--cap-add".to_owned(),
            "NET_ADMIN".to_owned(),
            "--env".to_owned(),
            "AP_AGENT_MODE=1".to_owned(),
            "--env".to_owned(),
            "GARDR_AGENT_MODE=1".to_owned(),
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
    for mount in &spec.runtime_mounts {
        let kind = match mount.kind {
            RuntimeMountType::Bind => "bind",
            RuntimeMountType::Volume => "volume",
        };
        let readonly = if mount.read_only { ",readonly" } else { "" };
        args.extend([
            "--mount".to_owned(),
            format!(
                "type={kind},source={},target={}{}",
                mount.source, mount.target, readonly
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
    if matches!(resolved_network(spec), Network::Bridge) {
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
        injected.push("--append-system-prompt".to_owned());
        injected.push("/gardr-context/AGENTS.md".to_owned());
        if continuity_marker.is_file() {
            injected.push("--append-system-prompt".to_owned());
            injected.push("/gardr-context/CONTINUITY.md".to_owned());
        }
    }
    command.splice(1..1, injected);
    if input.is_file() {
        command.extend(["-p".to_owned(), "@/gardr-input/task.md".to_owned()]);
    }
    args.extend(spec.startup_command.iter().cloned());
    args.extend(command);
    Ok(args)
}

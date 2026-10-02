use crate::*;
use std::fs::{self};
use std::path::PathBuf;

/// Container path where the pi adapter's writable transcript directory is mounted.
pub(crate) const PI_TRANSCRIPT_MOUNT: &str = "/gardr-transcript";

pub(crate) fn pi_runtime_domains(spec: &Spec) -> &'static [&'static str] {
    let provider = spec
        .harness
        .model
        .as_deref()
        .and_then(|model| model.split_once('/').map(|(provider, _)| provider));
    match provider {
        Some("anthropic") => &["api.anthropic.com"],
        Some("openai-codex") => &["api.openai.com", "chatgpt.com"],
        _ => &[],
    }
}

pub(crate) fn validate_pi_model(value: &str) -> Result<()> {
    let (provider, model) = value
        .split_once('/')
        .ok_or_else(|| "Pi model must be provider-qualified".to_owned())?;
    if provider.is_empty()
        || model.is_empty()
        || !provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/' | b':')
        })
    {
        return Err(format!("invalid Pi model: {value}"));
    }
    if !matches!(provider, "anthropic" | "openai-codex") {
        return Err(format!("unsupported Pi model provider: {provider}"));
    }
    Ok(())
}

pub(crate) fn sync_pi_auth(store: &Store, spec: &Spec) -> Result<()> {
    if !matches!(spec.harness.adapter, Some(Adapter::Pi)) {
        return Ok(());
    }
    if store.pi_agent_path().join("auth.json").exists() {
        return validate_pi_agent(store);
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "unable to locate host Pi authentication: HOME is not set".to_owned())?;
    sync_pi_auth_from(store, PathBuf::from(home).join(".pi/agent/auth.json"))
}

pub(crate) fn validate_pi_agent(store: &Store) -> Result<()> {
    let agent = store.pi_agent_path();
    let directory = fs::symlink_metadata(&agent)
        .map_err(|_| "managed Pi authentication is unavailable".to_owned())?;
    if !directory.is_dir() || directory.file_type().is_symlink() {
        return Err("managed Pi authentication directory is invalid".to_owned());
    }
    let auth = agent.join("auth.json");
    let file = fs::symlink_metadata(&auth)
        .map_err(|_| "managed Pi authentication is unavailable".to_owned())?;
    if !file.is_file() || file.file_type().is_symlink() {
        return Err("managed Pi authentication file is invalid".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if directory.permissions().mode() & 0o077 != 0 || file.permissions().mode() & 0o077 != 0 {
            return Err("managed Pi authentication permissions are not private".to_owned());
        }
    }
    Ok(())
}

pub(crate) fn sync_pi_auth_from(store: &Store, source: PathBuf) -> Result<()> {
    let auth = fs::read(&source).map_err(|error| {
        format!(
            "unable to read host Pi authentication {}: {error}",
            source.display()
        )
    })?;
    let agent = store.pi_agent_path();
    fs::create_dir_all(&agent).map_err(io_error)?;
    set_private_directory(&agent)?;
    let destination = agent.join("auth.json");
    atomic_write(&destination, &auth)?;
    set_private_file(&destination)
}

pub(crate) fn validate_harness_args(arguments: &[String]) -> Result<()> {
    if arguments.is_empty() {
        Ok(())
    } else {
        validate_command("harness arguments", arguments)
    }
}
pub(crate) fn validate_dispatch_harness_args(spec: &Spec, arguments: &[String]) -> Result<()> {
    if matches!(spec.harness.adapter, Some(Adapter::Pi))
        && arguments.iter().any(|argument| {
            is_pi_model_selection_argument(argument) || is_pi_session_management_argument(argument)
        })
    {
        return Err(
            "Pi dispatch arguments must not select a model, provider, or session".to_owned(),
        );
    }
    Ok(())
}
pub(crate) fn is_pi_reserved_argument(argument: &str) -> bool {
    matches!(argument, "-p" | "--print" | "--")
        || is_pi_model_selection_argument(argument)
        || is_pi_session_management_argument(argument)
}
fn is_pi_model_selection_argument(argument: &str) -> bool {
    matches!(
        argument,
        "--model" | "--provider" | "--api-key" | "--thinking"
    ) || ["--model=", "--provider=", "--api-key=", "--thinking="]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
}
/// Session-management arguments Gardr must own for the pi adapter, since it manages the
/// transcript itself. `--no-session` is deliberately excluded: existing specs may still declare
/// it in `harness.command` and Gardr silently drops it rather than requiring a rewrite.
///
/// `--name`/`-n` (session display name) is deliberately excluded too: it sets a label on the
/// session Gardr already selects via the injected `--session`, and does not conflict with it.
/// Rejecting it would break specs that were valid before Gardr started injecting `--session`.
fn is_pi_session_management_argument(argument: &str) -> bool {
    matches!(
        argument,
        "--session"
            | "--session-id"
            | "--session-dir"
            | "--fork"
            | "--continue"
            | "-c"
            | "--resume"
            | "-r"
    ) || ["--session=", "--session-id=", "--session-dir=", "--fork="]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
}

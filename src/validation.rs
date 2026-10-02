use crate::*;
use std::path::{Path, PathBuf};

pub fn validate_workspace(path: &Path) -> Result<PathBuf> {
    let workspace = path
        .canonicalize()
        .map_err(|error| format!("repository path {} is unavailable: {error}", path.display()))?;
    if !workspace.is_dir() {
        return Err(format!(
            "repository path must be a directory: {}",
            path.display()
        ));
    }
    Ok(workspace)
}

pub(crate) fn validate_name(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_environment_reference(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
    {
        Err(format!("invalid credential environment reference: {value}"))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_domain(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        Err(format!("invalid firewall domain: {value}"))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_command(label: &str, command: &[String]) -> Result<()> {
    if command.is_empty()
        || command.iter().any(|word| {
            word.is_empty() || word.contains('\0') || word.contains('\n') || word.contains('\r')
        })
    {
        Err(format!("invalid {label}"))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_container_path(label: &str, value: &str) -> Result<()> {
    if !value.starts_with('/')
        || value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value.contains('\0')
        || value.contains(',')
        || value.contains('\n')
        || value.contains('\r')
    {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}
pub(crate) fn validate_docker_path(label: &str, value: &Path) -> Result<()> {
    let value = value.to_string_lossy();
    if value.contains(',') || value.contains('\n') || value.contains('\r') || value.contains('\0') {
        Err(format!("invalid {label}: {value}"))
    } else {
        Ok(())
    }
}

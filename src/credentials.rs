use crate::*;
use serde::{Deserialize, Serialize};
use std::fs::{self};
use std::io;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    #[serde(default)]
    pub environment: Vec<CredentialRef>,
}
/// One `[credentials] environment` entry: either a bare env-var name (backward-compatible plain
/// string form) or a table mapping an in-container env var `name` to a credential-store key
/// `from` (defaulting to `name` when omitted).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CredentialRef {
    Name(String),
    Mapped(MappedCredential),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappedCredential {
    pub name: String,
    #[serde(default)]
    pub from: Option<String>,
}
impl CredentialRef {
    /// The in-container environment variable name.
    pub fn env_name(&self) -> &str {
        match self {
            CredentialRef::Name(name) => name,
            CredentialRef::Mapped(mapped) => &mapped.name,
        }
    }
    /// The credential-store key to resolve the value from; defaults to `env_name()`.
    pub fn registry_key(&self) -> &str {
        match self {
            CredentialRef::Name(name) => name,
            CredentialRef::Mapped(mapped) => mapped.from.as_deref().unwrap_or(&mapped.name),
        }
    }
}
/// Rejects credential values that would be unsafe to write verbatim into an `--env-file`'s
/// `NAME=value` line: Docker splits `--env-file` on newlines and treats a leading `#` as a
/// comment, with no quoting support, so an embedded newline (e.g. a pasted PEM/SSH key) would
/// inject additional, attacker/user-uncontrolled env assignments into the container and truncate
/// the real secret. A single trailing newline (common from files written by editors) is tolerated
/// since it is stripped before the value is written to the env file.
fn validate_credential_value(value: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(value)
        .map_err(|error| format!("credential value is not UTF-8: {error}"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.contains('\n') {
        return Err(
            "credential value must be a single line: multi-line values (e.g. a pasted PEM/SSH \
             key) cannot be safely stored in an env-file"
                .to_owned(),
        );
    }
    if text.starts_with('#') {
        return Err(
            "credential value must not begin with '#': env-files treat a leading '#' as a comment"
                .to_owned(),
        );
    }
    Ok(())
}
/// Best-effort removal of the run's plaintext resolved-credentials env-file. Missing is not an
/// error (e.g. the spec has no credentials, or cleanup already removed it).
pub(crate) fn remove_credentials_env(directory: &Path) {
    let path = directory.join("credentials.env");
    if let Err(error) = fs::remove_file(&path)
        && error.kind() != io::ErrorKind::NotFound
    {
        let _ = append_log(
            directory,
            &format!("failed to remove {}: {error}", path.display()),
        );
    }
}

impl Store {
    /// Registers (sets/upserts) a credential value under the private store. The value is never
    /// echoed back by any command.
    pub fn set_credential(&self, name: &str, value: &[u8]) -> Result<()> {
        validate_credential_value(value)?;
        let path = self.credential_path(name)?;
        create_private_dir_all(&self.credentials_path())?;
        atomic_write(&path, value)
    }

    /// Lists registered credential names. Values are never included.
    pub fn list_credentials(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        if !self.credentials_path().exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(self.credentials_path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            if entry.file_type().map_err(io_error)?.is_file() {
                names.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn remove_credential(&self, name: &str) -> Result<()> {
        let path = self.credential_path(name)?;
        if !path.exists() {
            return Err(format!("credential does not exist: {name}"));
        }
        fs::remove_file(&path).map_err(io_error)
    }
}

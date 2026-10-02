use crate::*;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
pub(crate) fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
}
#[cfg(not(unix))]
pub(crate) fn set_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}
/// Creates `path` (and any missing parents) with mode 0700 set from the `mkdir` call itself,
/// rather than a normal `create_dir_all` followed by a later `chmod`, so a directory that will
/// hold secret material is never briefly world/group-accessible at the umask default.
#[cfg(unix)]
pub(crate) fn create_private_dir_all(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(io_error)
}
#[cfg(not(unix))]
pub(crate) fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(io_error)
}
#[cfg(unix)]
pub(crate) fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)
}
#[cfg(not(unix))]
pub(crate) fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
#[cfg(unix)]
pub(crate) fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(io_error)
}

pub(crate) fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    use std::io::Write;
    file.write_all(contents).map_err(io_error)
}
pub(crate) fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    // Append (not replace) a suffix that includes the current process ID: `validate_name` allows
    // `.` in stored names (e.g. a credential named `a.b`), so `path.with_extension("new")` could
    // silently collide with and clobber an unrelated sibling entry (e.g. `a`). Appending to the
    // full file name instead keeps the temp path derived from, but distinct from, every valid
    // stored name.
    let mut temporary_name = path
        .file_name()
        .expect("atomic_write path has a file name")
        .to_os_string();
    temporary_name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path.with_file_name(temporary_name);
    write_private(&temporary, contents)?;
    fs::rename(temporary, path).map_err(io_error)
}
/// Writes `contents` to a newly created `path` with mode 0600 set by the `open` call itself
/// (rather than a normal write followed by a later `chmod`), so files that may hold secret
/// material (credentials, the run's resolved `credentials.env`) are never briefly readable at the
/// umask default. `atomic_write`'s callers that don't hold secrets (spec/run state, etc.) are
/// unaffected in practice other than also becoming private, which is harmless.
#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)?;
    file.write_all(contents).map_err(io_error)
}
#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).map_err(io_error)
}
pub(crate) fn approved_child(root: &Path, name: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("approved store root is unavailable: {error}"))?;
    let candidate = root.join(name);
    if fs::symlink_metadata(&candidate)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "approved store entry must not be a symlink: {name}"
        ));
    }
    let resolved = candidate.canonicalize().map_err(io_error)?;
    if !resolved.starts_with(&root) || !resolved.is_dir() {
        return Err(format!("approved store entry escapes its root: {name}"));
    }
    Ok(resolved)
}
pub(crate) fn digest_directory(directory: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_digest_files(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (path, contents) in files {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(contents);
        hash.update([0]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn collect_digest_files(
    base: &Path,
    directory: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_symlink() {
            return Err(format!(
                "image build context contains a symlink: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            collect_digest_files(base, &path, files)?;
        } else if kind.is_file() {
            files.push((
                path.strip_prefix(base)
                    .map_err(|error| error.to_string())?
                    .display()
                    .to_string(),
                fs::read(path).map_err(io_error)?,
            ));
        } else {
            return Err(format!(
                "image build context contains unsupported entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}
pub(crate) fn digest(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

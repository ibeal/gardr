use crate::*;
use serde::{Deserialize, Serialize};
use std::fs::{self};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub(crate) struct MountLock {
    name: String,
    source: String,
    sha256: String,
}
pub(crate) fn lock_mounts(store: &Store, spec: &Spec) -> Result<Vec<MountLock>> {
    spec.mounts
        .iter()
        .map(|mount| {
            let source = approved_child(&store.mounts_path(), &mount.name)?;
            Ok(MountLock {
                name: mount.name.clone(),
                source: source.display().to_string(),
                sha256: digest_directory(&source)?,
            })
        })
        .collect()
}
pub(crate) fn verify_locked_mounts(store: &Store, spec: &Spec, directory: &Path) -> Result<()> {
    let locked: Vec<MountLock> =
        serde_json::from_slice(&fs::read(directory.join("mounts.json")).map_err(io_error)?)
            .map_err(|error| format!("invalid resolved mount lock: {error}"))?;
    let current = lock_mounts(store, spec)?;
    if locked.len() != current.len()
        || locked.iter().zip(current.iter()).any(|(left, right)| {
            left.name != right.name || left.source != right.source || left.sha256 != right.sha256
        })
    {
        return Err("approved mounts changed since the run was created".to_owned());
    }
    Ok(())
}

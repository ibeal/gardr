use crate::*;
use std::fs::{self};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::Mutex;
pub(crate) static SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(crate) static PATH_LOCK: Mutex<()> = Mutex::new(());

/// Installs a fake `docker` executable at the front of `PATH` for the duration of the guard,
/// restoring the previous `PATH` on drop. Serialized via `PATH_LOCK` since `PATH` is
/// process-global.
pub(crate) struct FakeDocker {
    _guard: std::sync::MutexGuard<'static, ()>,
    original_path: Option<std::ffi::OsString>,
}
impl FakeDocker {
    pub(crate) fn install(script: &str, bin_dir: &Path) -> Self {
        let guard = PATH_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        fs::create_dir_all(bin_dir).unwrap();
        let docker = bin_dir.join("docker");
        fs::write(&docker, script).unwrap();
        set_executable(&docker).unwrap();
        let original_path = std::env::var_os("PATH");
        let mut new_path = bin_dir.as_os_str().to_owned();
        if let Some(existing) = &original_path {
            new_path.push(":");
            new_path.push(existing);
        }
        unsafe {
            std::env::set_var("PATH", new_path);
        }
        Self {
            _guard: guard,
            original_path,
        }
    }
}
impl Drop for FakeDocker {
    fn drop(&mut self) {
        unsafe {
            match &self.original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}
/// Builds `RuntimeOverrides` supplying only `--workspace`, for tests exercising a spec that
/// already sets image/harness/model itself.
pub(crate) fn overrides_for(workspace: &Path) -> RuntimeOverrides {
    RuntimeOverrides {
        workspace: Some(workspace.display().to_string()),
        ..Default::default()
    }
}
pub(crate) fn temporary_directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gardr-test-{}-{}",
        now(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}
pub(crate) fn spec() -> &'static [u8] {
    b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n"
}
pub(crate) fn directory_has_spec_toml(store: &Store, run_id: &str) -> bool {
    store.run_path(run_id).unwrap().join("spec.toml").is_file()
}

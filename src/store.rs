use crate::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn specs_path(&self) -> PathBuf {
        self.root.join("specs")
    }
    pub fn runs_path(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub fn workspaces_path(&self) -> PathBuf {
        self.root.join("workspaces")
    }
    pub fn threads_path(&self) -> PathBuf {
        self.root.join("threads")
    }
    pub fn base_context_path(&self) -> PathBuf {
        self.root.join("agent").join("AGENTS.md")
    }
    pub fn mounts_path(&self) -> PathBuf {
        self.root.join("mounts")
    }
    pub fn images_path(&self) -> PathBuf {
        self.root.join("images")
    }
    pub fn image_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("image profile name", name)?;
        Ok(self.images_path().join(format!("{name}.toml")))
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
}

impl Store {
    pub fn pi_agent_path(&self) -> PathBuf {
        self.root.join("pi").join("agent")
    }
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join("credentials")
    }
    pub fn credential_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("credential name", name)?;
        Ok(self.credentials_path().join(name))
    }
    pub fn spec_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("spec name", name)?;
        Ok(self.specs_path().join(format!("{name}.toml")))
    }
    pub fn run_path(&self, id: &str) -> Result<PathBuf> {
        validate_name("run id", id)?;
        Ok(self.runs_path().join(id))
    }
}

use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn open(path: PathBuf) -> Result<Self> {
        Ok(Self { path })
    }

    pub fn migrate(&self) -> Result<()> {
        Ok(())
    }

    pub fn reconcile_storage(&self, _data_dir: &Path) -> Result<()> {
        let _ = &self.path;
        Ok(())
    }
}


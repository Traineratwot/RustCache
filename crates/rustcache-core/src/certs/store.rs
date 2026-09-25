//! Certificate store helpers (paths + PEM IO).

use std::path::{Path, PathBuf};

use crate::{Error, Result};

pub struct CertStore {
    dir: PathBuf,
}

impl CertStore {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ca_cert_path(&self) -> PathBuf {
        self.dir.join("ca.crt")
    }

    pub fn ca_key_path(&self) -> PathBuf {
        self.dir.join("ca.key")
    }

    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    pub fn read_ca_cert_pem(&self) -> Result<String> {
        std::fs::read_to_string(self.ca_cert_path())
            .map_err(|e| Error::Cert(format!("read ca.crt: {e}")))
    }
}

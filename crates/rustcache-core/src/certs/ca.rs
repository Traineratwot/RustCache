//! Root CA generation and loading. Private key must be mode 0600.

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, SanType,
};

use crate::{Error, Result};

pub struct CaMaterial {
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

impl CaMaterial {
    /// Ensure key file is 0600 and cert is readable.
    pub fn harden_key_permissions(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&self.key_path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&self.key_path, perms)?;
        Ok(())
    }
}

/// Generate a new root CA into `dir` (`ca.crt` + `ca.key`).
pub fn generate_ca(dir: impl AsRef<Path>) -> Result<CaMaterial> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;

    let mut params =
        CertificateParams::new(Vec::<String>::new()).map_err(|e| Error::Cert(e.to_string()))?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "RustCache MITM Root");
    dn.push(DnType::OrganizationName, "RustCache");
    params.distinguished_name = dn;
    // Empty SAN list is fine for a CA; add a dummy IP-less SAN if required by rcgen.
    params.subject_alt_names = vec![SanType::DnsName(
        "RustCache MITM Root CA"
            .try_into()
            .map_err(|e| Error::Cert(format!("dns name: {e:?}")))?,
    )];

    let key = KeyPair::generate().map_err(|e| Error::Cert(e.to_string()))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| Error::Cert(e.to_string()))?;

    let cert_pem = cert.pem();
    let key_pem = key.serialize_pem();

    let cert_path = dir.join("ca.crt");
    let key_path = dir.join("ca.key");
    std::fs::write(&cert_path, &cert_pem)?;
    use std::os::unix::fs::OpenOptionsExt;
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)?;
        std::io::Write::write_all(&mut f, key_pem.as_bytes())?;
    }

    let material = CaMaterial {
        cert_pem,
        key_pem,
        cert_path,
        key_path,
    };
    material.harden_key_permissions()?;
    Ok(material)
}

/// Load existing CA or generate one if missing.
pub fn load_ca(dir: impl AsRef<Path>) -> Result<CaMaterial> {
    let dir = dir.as_ref();
    let cert_path = dir.join("ca.crt");
    let key_path = dir.join("ca.key");
    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path)?;
        let key_pem = std::fs::read_to_string(&key_path)?;
        let material = CaMaterial {
            cert_pem,
            key_pem,
            cert_path,
            key_path,
        };
        material.harden_key_permissions()?;
        return Ok(material);
    }
    generate_ca(dir)
}

/// PEM bytes of the root certificate.
pub fn export_pem(material: &CaMaterial) -> Vec<u8> {
    material.cert_pem.as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_load_ca() {
        let dir = std::env::temp_dir().join(format!("rc-ca-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        assert!(ca.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(ca.key_pem.contains("PRIVATE KEY"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&ca.key_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let again = load_ca(&dir).unwrap();
        assert_eq!(again.cert_pem, ca.cert_pem);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

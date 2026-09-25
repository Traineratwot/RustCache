//! On-the-fly leaf certificates for MITM, cached in a DashMap.

use std::sync::Arc;

use dashmap::DashMap;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};

use super::ca::CaMaterial;
use crate::{Error, Result};

#[derive(Clone)]
pub struct LeafCert {
    pub cert_pem: String,
    pub key_pem: String,
}

type CaIssuer = Issuer<'static, KeyPair>;

pub struct LeafIssuer {
    issuer: CaIssuer,
    cache: DashMap<String, Arc<LeafCert>>,
}

impl LeafIssuer {
    pub fn from_ca(ca: &CaMaterial) -> Result<Self> {
        let key =
            KeyPair::from_pem(&ca.key_pem).map_err(|e| Error::Cert(format!("ca key: {e}")))?;
        let issuer = Issuer::from_ca_cert_pem(&ca.cert_pem, key)
            .map_err(|e| Error::Cert(format!("ca issuer: {e}")))?;
        Ok(Self {
            issuer,
            cache: DashMap::new(),
        })
    }

    pub fn issue(&self, host: &str) -> Result<Arc<LeafCert>> {
        if let Some(hit) = self.cache.get(host) {
            return Ok(hit.value().clone());
        }
        let leaf = Arc::new(sign_leaf(&self.issuer, host)?);
        self.cache.insert(host.to_string(), leaf.clone());
        Ok(leaf)
    }

    pub fn cached_hosts(&self) -> usize {
        self.cache.len()
    }
}

fn sign_leaf(issuer: &CaIssuer, host: &str) -> Result<LeafCert> {
    let mut params =
        CertificateParams::new(vec![host.to_string()]).map_err(|e| Error::Cert(e.to_string()))?;
    params.subject_alt_names = vec![SanType::DnsName(
        host.try_into()
            .map_err(|e| Error::Cert(format!("dns: {e:?}")))?,
    )];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, host);
    params.distinguished_name = dn;

    let key = KeyPair::generate().map_err(|e| Error::Cert(e.to_string()))?;
    let cert = params
        .signed_by(&key, issuer)
        .map_err(|e| Error::Cert(format!("sign leaf: {e}")))?;

    Ok(LeafCert {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::ca::generate_ca;

    #[test]
    fn issue_leaf_for_host() {
        let dir = std::env::temp_dir().join(format!("rc-leaf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        let issuer = LeafIssuer::from_ca(&ca).unwrap();
        let leaf = issuer.issue("example.com").unwrap();
        assert!(leaf.cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(issuer.issue("example.com").unwrap().cert_pem, leaf.cert_pem);
        assert_eq!(issuer.cached_hosts(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaf_parses_and_embeds_san_host() {
        let dir = std::env::temp_dir().join(format!("rc-leaf-san-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        let issuer = LeafIssuer::from_ca(&ca).unwrap();
        let host = "shop.example.org";
        let leaf = issuer.issue(host).unwrap();

        // parse-back PEM → DER
        let mut certs = rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(certs.len(), 1);
        let der = certs.pop().unwrap();
        // DNS name / CN appears in the DER-encoded SAN / subject
        assert!(
            der.as_ref()
                .windows(host.len())
                .any(|w| w == host.as_bytes()),
            "host must appear in cert DER (SAN/CN)"
        );

        // key PEM parses
        let keys = rustls_pemfile::private_key(&mut leaf.key_pem.as_bytes()).unwrap();
        assert!(keys.is_some());

        // leaf is not the CA cert
        assert_ne!(leaf.cert_pem, ca.cert_pem);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn distinct_hosts_get_distinct_certs() {
        let dir = std::env::temp_dir().join(format!("rc-leaf-dist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        let issuer = LeafIssuer::from_ca(&ca).unwrap();
        let a = issuer.issue("a.example.com").unwrap();
        let b = issuer.issue("b.example.com").unwrap();
        assert_ne!(a.cert_pem, b.cert_pem);
        assert_eq!(issuer.cached_hosts(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

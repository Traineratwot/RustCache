//! On-the-fly leaf certificates for MITM, cached in a bounded DashMap.

use std::sync::Arc;

use dashmap::DashMap;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair};
use time::{Duration, OffsetDateTime};

use super::ca::CaMaterial;
use crate::{Error, Result};

/// Maximum number of host → leaf-cert entries kept in memory.
///
/// Prevents unbounded growth when clients probe many SNI names.
const MAX_CACHED_LEAVES: usize = 512;

/// Leaf lifetime. Apple platforms reject TLS server certificates valid for more
/// than 398 days, so rcgen's default (`1975..4096`) makes every MITM handshake
/// fail on macOS/iOS clients. Leaves are cached in memory only and re-minted on
/// restart, so a short window costs nothing.
const LEAF_VALID_DAYS: i64 = 397;

/// Backdate slightly so a client whose clock is a little behind still accepts
/// the freshly minted certificate.
const LEAF_BACKDATE_SECS: i64 = 3600;

/// A leaf certificate (PEM) and private key for one host.
#[derive(Clone)]
pub struct LeafCert {
    pub cert_pem: String,
    pub key_pem: String,
}

type CaIssuer = Issuer<'static, KeyPair>;

/// Issues and caches leaf certificates signed by the local CA.
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

    /// Return a cached leaf for `host`, minting and caching one if absent.
    pub fn issue(&self, host: &str) -> Result<Arc<LeafCert>> {
        if let Some(hit) = self.cache.get(host) {
            return Ok(hit.value().clone());
        }
        // Bound the cache: drop everything when full (simple, avoids LRU bookkeeping
        // on a hot path). Hosts are re-minted cheaply on next use.
        if self.cache.len() >= MAX_CACHED_LEAVES {
            self.cache.clear();
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
    // `CertificateParams::new` already classifies the SAN: an IP literal becomes
    // `SanType::IpAddress`, anything else a DNS name. Overwriting it with a bare
    // `DnsName` produced certificates no client accepts for `https://127.0.0.1/`.
    let mut params =
        CertificateParams::new(vec![host.to_string()]).map_err(|e| Error::Cert(e.to_string()))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, host);
    params.distinguished_name = dn;

    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::seconds(LEAF_BACKDATE_SECS);
    params.not_after = now + Duration::days(LEAF_VALID_DAYS);

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
    fn leaf_validity_is_client_acceptable() {
        let dir = std::env::temp_dir().join(format!("rc-leaf-valid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        let issuer = LeafIssuer::from_ca(&ca).unwrap();
        let leaf = issuer.issue("example.com").unwrap();

        let der = rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let (_, parsed) = x509_parser::parse_x509_certificate(der.as_ref()).unwrap();
        let validity = parsed.validity();
        let days = (validity.not_after.timestamp() - validity.not_before.timestamp()) / 86_400;
        // Apple platforms reject server certs valid for more than 398 days.
        assert!(days <= 398, "leaf valid for {days} days");
        assert!(days >= 300, "leaf valid for only {days} days");
    }

    #[test]
    fn ip_literal_host_gets_an_ip_san() {
        let dir = std::env::temp_dir().join(format!("rc-leaf-ip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = generate_ca(&dir).unwrap();
        let issuer = LeafIssuer::from_ca(&ca).unwrap();
        let leaf = issuer.issue("127.0.0.1").unwrap();

        let der = rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let (_, parsed) = x509_parser::parse_x509_certificate(der.as_ref()).unwrap();
        let san = parsed
            .subject_alternative_name()
            .unwrap()
            .expect("SAN extension");
        assert!(
            san.value
                .general_names
                .iter()
                .any(|gn| matches!(gn, x509_parser::extensions::GeneralName::IPAddress(_))),
            "IP host must yield an iPAddress SAN, got {:?}",
            san.value.general_names
        );
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

//! National CA roots shipped with the binary.
//!
//! `webpki-roots` is Mozilla's bundle and does not carry national trust
//! anchors such as Russian Trusted CA, which Russian banks (Sber, T-Bank,
//! Alfa, VTB, …) chain to. Rather than requiring an operator to install
//! those CAs into the host store first, the PEMs under `assets/ca/` are
//! embedded and installed into the upstream root store at startup — but
//! only when no equivalent anchor is already trusted (system store or
//! `webpki-roots`), so an explicit host install is never silently shadowed.

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;

/// label, PEM text. Order: root first, then its intermediates.
const BUNDLED_ROOTS: &[(&str, &str)] = &[
    (
        "Russian Trusted Root CA",
        include_str!("../assets/ca/russian_trusted_root_ca.crt"),
    ),
    (
        "Russian Trusted Sub CA",
        include_str!("../assets/ca/russian_trusted_sub_ca.crt"),
    ),
];

/// Outcome of [`install_bundled_roots`], for startup logging and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RootInstall {
    /// Installed from the embedded PEMs.
    pub added: Vec<&'static str>,
    /// An equivalent trust anchor was already present — left untouched.
    pub present: Vec<&'static str>,
    /// PEM parsed but rustls rejected it; nothing was added.
    pub failed: Vec<&'static str>,
}

/// Install embedded national CA roots into `store`, skipping any that are
/// already trusted under the same subject + public key.
pub fn install_bundled_roots(store: &mut RootCertStore) -> RootInstall {
    let mut report = RootInstall::default();
    for &(name, pem) in BUNDLED_ROOTS {
        match install_one(store, pem) {
            Ok(true) => report.added.push(name),
            Ok(false) => report.present.push(name),
            Err(err) => {
                tracing::warn!(cert = name, error = %err, "bundled root not installed");
                report.failed.push(name);
            }
        }
    }
    report
}

/// Returns `Ok(true)` when installed, `Ok(false)` when already trusted.
fn install_one(store: &mut RootCertStore, pem: &str) -> anyhow::Result<bool> {
    let der = first_pem_cert(pem)?;
    // Extract the trust anchor through the same path rustls uses on `add`,
    // then dedup on subject+SPKI. A host copy of the same CA therefore wins
    // over the embedded one and the store never gets a double anchor.
    let mut scratch = RootCertStore::empty();
    scratch.add(der)?;
    let anchor = scratch
        .roots
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("PEM produced no trust anchor"))?;
    if store.roots.iter().any(|ta| {
        ta.subject == anchor.subject && ta.subject_public_key_info == anchor.subject_public_key_info
    }) {
        return Ok(false);
    }
    store.roots.push(anchor);
    Ok(true)
}

fn first_pem_cert(pem: &str) -> anyhow::Result<CertificateDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let mut certs = rustls_pemfile::certs(&mut reader);
    certs
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("no certificate in PEM"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_pems_each_hold_one_cert() {
        for &(name, pem) in BUNDLED_ROOTS {
            let der = first_pem_cert(pem).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!der.as_ref().is_empty(), "{name} DER is empty");
        }
        assert_eq!(BUNDLED_ROOTS.len(), 2);
    }

    #[test]
    fn install_adds_when_store_has_nothing() {
        let mut store = RootCertStore::empty();
        let report = install_bundled_roots(&mut store);
        assert!(
            report.failed.is_empty(),
            "unexpected failures: {:?}",
            report.failed
        );
        assert_eq!(report.added.len(), BUNDLED_ROOTS.len());
        assert!(report.present.is_empty());
        assert_eq!(store.roots.len(), BUNDLED_ROOTS.len());
    }

    #[test]
    fn install_skips_when_anchor_already_present() {
        let mut store = RootCertStore::empty();
        let first = install_bundled_roots(&mut store);
        assert_eq!(first.added.len(), BUNDLED_ROOTS.len());
        let roots_after_first = store.roots.len();

        let second = install_bundled_roots(&mut store);
        assert!(second.added.is_empty(), "second run must not add");
        assert_eq!(second.present.len(), BUNDLED_ROOTS.len());
        assert_eq!(store.roots.len(), roots_after_first, "store must not grow");
    }

    #[test]
    fn install_skips_when_same_anchor_came_from_system_store() {
        // Host-installed copy: same DER loaded before the bundled ones.
        let mut store = RootCertStore::empty();
        let pem = BUNDLED_ROOTS[0].1;
        let der = first_pem_cert(pem).expect("root PEM");
        store.add(der).expect("system copy parses");

        let report = install_bundled_roots(&mut store);
        assert!(
            report.present.contains(&BUNDLED_ROOTS[0].0),
            "host copy must win: {:?}",
            report
        );
        assert!(!report.added.contains(&BUNDLED_ROOTS[0].0));
        // Sub CA is not in the store yet, so it is still installed.
        assert!(report.added.contains(&BUNDLED_ROOTS[1].0));
    }
}

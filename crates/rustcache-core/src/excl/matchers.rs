//! Domain / CIDR exclusion matchers.

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

/// One exclusion rule. Variants map to config syntax (`*.example.com`, `.com`, CIDR, bare host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    Exact {
        value: String,
    },
    Wildcard {
        value: String,
    },
    Suffix {
        value: String,
    },
    Cidr {
        value: String,
        /// Parsed form of `value`, filled on first IP match (avoids re-parsing).
        #[serde(skip)]
        net: OnceLock<IpNet>,
    },
}

impl Matcher {
    /// Parse a single exclusion spec into one or more matchers.
    pub fn parse(spec: &str) -> Vec<Matcher> {
        let s = spec.trim();
        if s.is_empty() {
            return vec![];
        }
        if let Ok(net) = s.parse::<IpNet>() {
            let slot = OnceLock::new();
            let _ = slot.set(net);
            return vec![Matcher::Cidr {
                value: s.to_string(),
                net: slot,
            }];
        }
        if let Some(rest) = s.strip_prefix("*.") {
            // Wildcard: foo.example.com matches, example.com does not.
            return vec![Matcher::Wildcard {
                value: rest.to_ascii_lowercase(),
            }];
        }
        if let Some(rest) = s.strip_prefix('.') {
            // Suffix: example.com and foo.example.com match.
            return vec![Matcher::Suffix {
                value: rest.to_ascii_lowercase(),
            }];
        }
        vec![Matcher::Exact {
            value: s.to_ascii_lowercase(),
        }]
    }

    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let host = host.split('%').next().unwrap_or(&host);
        match self {
            Matcher::Exact { value } => host == value.as_str(),
            Matcher::Wildcard { value } => {
                // *.example.com matches foo.example.com but not example.com
                host.len() > value.len() + 1
                    && host.ends_with(value.as_str())
                    && host.as_bytes()[host.len() - value.len() - 1] == b'.'
            }
            Matcher::Suffix { value } => {
                host == value.as_str()
                    || (host.len() > value.len() + 1
                        && host.ends_with(value.as_str())
                        && host.as_bytes()[host.len() - value.len() - 1] == b'.')
            }
            Matcher::Cidr { .. } => false,
        }
    }

    pub fn matches_ip(&self, ip: IpAddr) -> bool {
        match self {
            Matcher::Cidr { value, net } => {
                if let Some(parsed) = net.get() {
                    return parsed.contains(&ip);
                }
                match value.parse::<IpNet>() {
                    Ok(parsed) => {
                        let contains = parsed.contains(&ip);
                        let _ = net.set(parsed);
                        contains
                    }
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }

    pub fn matches_host_or_ip(&self, host: &str, ip: Option<IpAddr>) -> bool {
        if self.matches_host(host) {
            return true;
        }
        if let Some(ip) = ip {
            return self.matches_ip(ip);
        }
        // Also try host as IP literal
        if let Ok(ip) = host.parse::<IpAddr>() {
            return self.matches_ip(ip);
        }
        false
    }
}

/// Hot-swappable set of exclusions (domains + CIDRs).
///
/// Hold this behind an `Arc` / `RwLock` at the call site to hot-reload;
/// the set itself is immutable after construction.
#[derive(Debug, Clone, Default)]
pub struct ExclusionSet {
    matchers: Arc<Vec<Matcher>>,
}

impl ExclusionSet {
    pub fn new(matchers: Vec<Matcher>) -> Self {
        Self {
            matchers: Arc::new(matchers),
        }
    }

    pub fn from_specs(domains: &[String], cidrs: &[String]) -> Self {
        let mut ms = Vec::new();
        for d in domains {
            ms.extend(Matcher::parse(d));
        }
        for c in cidrs {
            ms.extend(Matcher::parse(c));
        }
        Self::new(ms)
    }

    pub fn is_excluded(&self, host: &str, ip: Option<IpAddr>) -> bool {
        self.matchers.iter().any(|m| m.matches_host_or_ip(host, ip))
    }

    pub fn is_excluded_url(&self, url: &str) -> bool {
        let host = extract_host(url);
        let ip = host.parse::<IpAddr>().ok();
        self.is_excluded(&host, ip)
    }

    pub fn matchers(&self) -> &[Matcher] {
        &self.matchers
    }
}

/// Host part of a URL, without scheme, port, userinfo or IPv6 brackets.
///
/// The brackets matter: `https://[::1]/` must yield `::1`, otherwise the CIDR
/// matchers never see a parsable address and IPv6 exclusions silently never fire.
fn extract_host(url: &str) -> String {
    let rest = match url.split_once("://") {
        Some((_, r)) => r,
        None => url,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Strip userinfo (`user:pass@host`) before looking for the port.
    let authority = match authority.rsplit_once('@') {
        Some((_, h)) => h,
        None => authority,
    };
    if let Some(inner) = authority.strip_prefix('[') {
        // IPv6 literal: everything up to the closing bracket is the host.
        if let Some((host, _port)) = inner.split_once(']') {
            return host.to_ascii_lowercase();
        }
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            h.to_ascii_lowercase()
        }
        _ => authority.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let s = ExclusionSet::from_specs(&["bank.example".into()], &[]);
        assert!(s.is_excluded("bank.example", None));
        assert!(!s.is_excluded("notbank.example", None));
        assert!(!s.is_excluded("x.bank.example", None));
    }

    #[test]
    fn wildcard_and_suffix() {
        let s = ExclusionSet::from_specs(&["*.local".into()], &[]);
        assert!(s.is_excluded("foo.local", None));
        assert!(s.is_excluded("a.b.local", None));
        assert!(!s.is_excluded("local", None));
        assert!(!s.is_excluded("notlocal", None));
    }

    #[test]
    fn suffix_matches_root_and_sub() {
        let s = ExclusionSet::new(vec![Matcher::Suffix {
            value: "example.com".into(),
        }]);
        assert!(s.is_excluded("example.com", None));
        assert!(s.is_excluded("a.example.com", None));
        assert!(!s.is_excluded("example.org", None));
    }

    #[test]
    fn cidr_match() {
        let s = ExclusionSet::from_specs(&[], &["10.0.0.0/8".into()]);
        assert!(s.is_excluded("10.1.2.3", None));
        assert!(s.is_excluded("host", Some("10.9.9.9".parse().unwrap())));
        assert!(!s.is_excluded("8.8.8.8", None));
    }

    #[test]
    fn negative_cases_do_not_match() {
        let s = ExclusionSet::from_specs(
            &["bank.example".into(), "*.local".into()],
            &["192.168.0.0/16".into()],
        );
        assert!(!s.is_excluded("notbank.example", None));
        assert!(!s.is_excluded("evil-local", None));
        assert!(!s.is_excluded("10.0.0.1", None));
        assert!(!s.is_excluded_url("http://example.org/"));
    }

    #[test]
    fn ipv6_literal_url_matches_cidr() {
        let s = ExclusionSet::from_specs(&[], &["fd00::/8".into()]);
        assert!(s.is_excluded_url("https://[fd00::1]:8443/x"));
        assert!(s.is_excluded_url("http://[fd00::2]/"));
        assert!(!s.is_excluded_url("http://[2001:db8::1]/"));
    }

    #[test]
    fn userinfo_is_not_mistaken_for_the_host() {
        let s = ExclusionSet::from_specs(&["bank.example".into()], &[]);
        assert!(s.is_excluded_url("https://user:pw@bank.example/login"));
        assert!(!s.is_excluded_url("https://bank.example@other.test/"));
    }

    #[test]
    fn is_excluded_url_extracts_host_and_port() {
        let s = ExclusionSet::from_specs(&["bank.example".into()], &[]);
        assert!(s.is_excluded_url("https://bank.example:8443/login"));
        assert!(!s.is_excluded_url("https://other.example/"));
    }
}

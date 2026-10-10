//! Canonical URL → blake3 hex cache key. Hex-only filenames prevent path traversal.

use std::fmt;

/// A cache key derived from a canonical URL (blake3 digest as lowercase hex).
///
/// Disk layout and any filesystem access keyed by cache identity must use
/// [`is_hex_key`] so a crafted key can never escape the cache root.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    hex: String,
}

impl CacheKey {
    pub fn as_str(&self) -> &str {
        &self.hex
    }

    /// 2-level fanout prefix: `ab/cd`.
    pub fn fanout(&self) -> (&str, &str) {
        fanout(&self.hex)
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex)
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CacheKey({})", &self.hex[..12.min(self.hex.len())])
    }
}

/// Canonicalize a request URL for cache-key purposes.
///
/// - Lowercase scheme and host
/// - Drop default ports (80/http, 443/https)
/// - Drop fragment
/// - Normalize empty path to `/`
/// - Sort query is left as-is (order is semantically significant for many origins)
///
/// Host parsing goes through `rustcache_util::split_authority` so IPv6
/// literals (`http://[::1]:8080/`) keep their brackets out of the host and the
/// default-port rule applies to the port, not to a slice of the address.
pub fn canonical_url(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest): (String, &str) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_string(), url),
    };

    let (authority, path_and_query) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };

    // split_authority strips userinfo and IPv6 brackets; we rebuild authority
    // from (host, port) below so the default-port drop is well-defined.
    let (raw_host, raw_port) = rustcache_util::split_authority(authority);
    let host = raw_host.to_ascii_lowercase();
    // Re-bracket IPv6 literals: `host` is bare `::1` after stripping.
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };

    let default_port = match scheme.as_str() {
        "https" => Some(443u16),
        "http" => Some(80),
        _ => None,
    };
    let authority = match raw_port {
        Some(p) => match p.parse::<u16>() {
            Ok(port) if Some(port) != default_port => format!("{host}:{port}"),
            // Default port, or unparseable port that is not a real port — drop it.
            _ => host,
        },
        None => host,
    };

    let path_and_query = path_and_query.split('#').next().unwrap_or("");
    match path_and_query.strip_prefix('?') {
        // Query-only target: normalize the empty path to `/`.
        Some(q) => format!("{scheme}://{authority}/?{q}"),
        None if path_and_query.is_empty() => format!("{scheme}://{authority}/"),
        None => format!("{scheme}://{authority}{path_and_query}"),
    }
}

/// blake3 hash of the canonical URL as lowercase hex (64 chars).
pub fn cache_key(url: &str) -> CacheKey {
    let canon = canonical_url(url);
    let hash = blake3::hash(canon.as_bytes());
    CacheKey {
        hex: hex::encode(hash.as_bytes()),
    }
}

/// blake3 hex of the canonical URL (owned convenience wrapper).
pub fn key_hex(url: &str) -> String {
    cache_key(url).hex.clone()
}

/// 2-level fanout prefix for a hex key string: `("ab", "cd")`.
///
/// Shared by [`CacheKey::fanout`] and the disk cache so path layout stays
/// consistent in one place.
pub fn fanout(key: &str) -> (&str, &str) {
    let b = key.as_bytes();
    if b.len() >= 4 {
        (
            std::str::from_utf8(&b[0..2]).unwrap_or("00"),
            std::str::from_utf8(&b[2..4]).unwrap_or("00"),
        )
    } else {
        ("00", "00")
    }
}

/// True when `key` is a 64-char ASCII hex digest (blake3).
///
/// Every disk path must call this before touching the filesystem — it is the
/// hard guard against path traversal via non-hex keys.
pub fn is_hex_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_lowercases_and_strips_default_port() {
        assert_eq!(
            canonical_url("HTTP://Example.COM:80/Path"),
            "http://example.com/Path"
        );
        assert_eq!(
            canonical_url("https://Example.COM:443/"),
            "https://example.com/"
        );
        assert_eq!(
            canonical_url("https://Example.COM:8443/"),
            "https://example.com:8443/"
        );
    }

    #[test]
    fn canonical_drops_fragment_and_empty_path() {
        assert_eq!(canonical_url("http://example.com"), "http://example.com/");
        assert_eq!(
            canonical_url("http://example.com/a/b#frag"),
            "http://example.com/a/b"
        );
    }

    #[test]
    fn key_is_hex_and_stable() {
        let k = cache_key("http://example.com/");
        assert_eq!(k.as_str().len(), 64);
        assert!(k.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(k.as_str(), cache_key("http://EXAMPLE.COM/").as_str());
    }

    #[test]
    fn fanout_uses_first_four_hex_chars() {
        let k = cache_key("http://example.com/");
        let (a, c) = k.fanout();
        assert_eq!(format!("{a}{c}"), &k.as_str()[..4]);
    }

    #[test]
    fn query_only_path_normalized() {
        assert_eq!(
            canonical_url("http://example.com?a=1"),
            "http://example.com/?a=1"
        );
    }

    #[test]
    fn non_default_port_preserved() {
        assert_eq!(
            canonical_url("http://example.com:8080/x"),
            "http://example.com:8080/x"
        );
    }

    #[test]
    fn ipv6_literal_keeps_brackets_and_drops_default_port() {
        // Regression: a naive `rsplit_once(':')` treated `::1` as host `:` + port `1`.
        assert_eq!(canonical_url("http://[::1]:80/x"), "http://[::1]/x");
        assert_eq!(
            canonical_url("http://[2001:db8::1]:8080/a"),
            "http://[2001:db8::1]:8080/a"
        );
        assert_eq!(canonical_url("https://[::1]:443/"), "https://[::1]/");
    }

    #[test]
    fn different_urls_different_keys() {
        let a = cache_key("http://example.com/a");
        let b = cache_key("http://example.com/b");
        assert_ne!(a.as_str(), b.as_str());
    }
}

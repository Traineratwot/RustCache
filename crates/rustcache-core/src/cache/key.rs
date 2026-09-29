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

    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_ascii_lowercase(), Some(p))
        }
        _ => (authority.to_ascii_lowercase(), None),
    };

    let default_port = match scheme.as_str() {
        "https" => "443",
        "http" => "80",
        _ => "",
    };
    let authority = match port {
        Some(p) if !p.is_empty() && p != default_port => format!("{host}:{p}"),
        _ => host,
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
    fn different_urls_different_keys() {
        let a = cache_key("http://example.com/a");
        let b = cache_key("http://example.com/b");
        assert_ne!(a.as_str(), b.as_str());
    }
}

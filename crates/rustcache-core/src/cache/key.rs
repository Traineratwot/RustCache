//! Canonical URL → blake3 hex cache key. Hex-only filenames prevent path traversal.

use std::fmt;

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
        let b = self.hex.as_bytes();
        // key is always 64 hex chars (blake3)
        let a = std::str::from_utf8(&b[0..2]).unwrap_or("00");
        let c = std::str::from_utf8(&b[2..4]).unwrap_or("00");
        (a, c)
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
    let path_and_query = if path_and_query.is_empty() {
        "/"
    } else if path_and_query.starts_with('?') {
        // path empty, query only
        &format!("/{path_and_query}")[1..] // keep as-is with leading ? — normalize to /?...
    } else {
        path_and_query
    };

    if let Some(q) = path_and_query.strip_prefix('?') {
        format!("{scheme}://{authority}/?{q}")
    } else {
        format!("{scheme}://{authority}{path_and_query}")
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

pub fn key_hex(url: &str) -> String {
    cache_key(url).hex.clone()
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
}

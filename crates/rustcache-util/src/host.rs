//! Authority / URL host parsing.
//!
//! Every place that understands `Host:`, CONNECT targets, PAC URLs or absolute
//! request URIs needs the same three rules:
//!
//! 1. IPv6 literals are bracketed (`[::1]:443`); the brackets are *not* part of
//!    the host. A bare `::1:443` is ambiguous (is `443` a port or part of the
//!    address?) and is rejected where a port is required.
//! 2. The port separator is the *last* colon, and only when the suffix is all
//!    digits — otherwise the colon belongs to the host (bare IPv6, or a
//!    userinfo-looking string that was not stripped).
//! 3. Userinfo (`user:pass@host`) is never a host. URLs may carry it; headers
//!    should not, but we strip it defensively so exclusions cannot be bypassed
//!    with `https://victim@bank.example/`.
//!
//! The zero-copy [`split_authority`] is the primitive. The owned
//! [`split_host_port`] / [`split_host_port_or`] wrappers add a required or
//! default port for dialers. [`host_from_url`] is the URL-shaped entry point
//! (scheme / path / userinfo stripping).

use std::fmt;

/// Host or port of an authority could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostParseError {
    input: String,
    reason: &'static str,
}

impl HostParseError {
    fn new(input: &str, reason: &'static str) -> Self {
        Self {
            input: input.to_string(),
            reason,
        }
    }
}

impl fmt::Display for HostParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad host/port `{}`: {}", self.input, self.reason)
    }
}

impl std::error::Error for HostParseError {}

/// Split an authority into `(host, port)` without allocating.
///
/// Accepts `example.com`, `example.com:443`, `[::1]`, `[::1]:443`.
/// The returned host never includes IPv6 brackets. `port` is `None` when the
/// authority has no port (`example.com`, `[::1]`) or an empty one (`host:`).
///
/// A bare IPv6 address without brackets (`::1`) is returned whole as the host
/// with no port — callers that need a port should use [`split_host_port`],
/// which rejects that form as ambiguous.
pub fn split_authority(authority: &str) -> (&str, Option<&str>) {
    let authority = strip_userinfo(authority);
    if let Some(inner) = authority.strip_prefix('[') {
        match inner.split_once(']') {
            Some((host, rest)) => {
                let port = rest.strip_prefix(':').filter(|p| !p.is_empty());
                (host, port)
            }
            // Unclosed bracket: treat the whole input as the host so we do not
            // silently drop half of it.
            None => (authority, None),
        }
    } else {
        match authority.rsplit_once(':') {
            // Only treat the suffix as a port when:
            // - the host part is non-empty and has no further colons (so a bare
            //   IPv6 like `::1` or `2001:db8::1` stays intact), and
            // - the suffix is a non-empty run of digits (so `host:` and
            //   `host:notaport` are not split).
            Some((h, p))
                if !h.is_empty()
                    && !h.contains(':')
                    && !p.is_empty()
                    && p.chars().all(|c| c.is_ascii_digit()) =>
            {
                (h, Some(p))
            }
            _ => (authority, None),
        }
    }
}

/// Host part of an authority: no port, no IPv6 brackets, userinfo stripped.
///
/// The result is *not* lowercased — callers doing hostname comparison should
/// use `eq_ignore_ascii_case` so internationalised or mixed-case names work.
pub fn host_of(authority: &str) -> &str {
    split_authority(authority).0
}

/// Split `host:port` where the port is required. Returns owned strings.
///
/// Handles the socket-address fast path (`127.0.0.1:80`, `[::1]:80`) and the
/// bracketed / dotted forms. Bare IPv6 without brackets is rejected: `::1`
/// cannot be told apart from `host:port` without a grammar the caller did not
/// ask for.
pub fn split_host_port(addr: &str) -> Result<(String, u16), HostParseError> {
    // SocketAddr accepts `v4:port` and `[v6]:port` and rejects everything else
    // quickly — covers the common case with zero string work.
    if let Ok(sa) = addr.parse::<std::net::SocketAddr>() {
        return Ok((sa.ip().to_string(), sa.port()));
    }
    let (host, port) = split_authority(addr);
    if host.is_empty() {
        return Err(HostParseError::new(addr, "empty host"));
    }
    let Some(port) = port else {
        return Err(HostParseError::new(addr, "missing port"));
    };
    let port: u16 = port
        .parse()
        .map_err(|_| HostParseError::new(addr, "port is not a u16"))?;
    Ok((host.to_string(), port))
}

/// Split `host:port`, falling back to `default_port` when the port is absent.
///
/// Used for CONNECT targets (`example.com` → 443) and HTTP absolute-form
/// URIs (`example.com` → 80). An explicitly empty port (`host:`) also falls
/// back to the default rather than failing.
pub fn split_host_port_or(
    target: &str,
    default_port: u16,
) -> Result<(String, u16), HostParseError> {
    let (host, port) = split_authority(target);
    if host.is_empty() {
        return Err(HostParseError::new(target, "empty host"));
    }
    match port {
        Some(p) => {
            let port: u16 = p
                .parse()
                .map_err(|_| HostParseError::new(target, "port is not a u16"))?;
            Ok((host.to_string(), port))
        }
        None => Ok((host.to_string(), default_port)),
    }
}

/// Host of a URL, absolute-form target or bare authority — lowercased, without
/// scheme, userinfo, port, path or IPv6 brackets.
///
/// This is the form exclusion matchers and cache keys want:
/// `https://User@WWW.Example.com:443/a?b` → `www.example.com`.
/// `https://[::1]/` → `::1` (CIDR matchers need the bare literal).
pub fn host_from_url(url: &str) -> String {
    authority_of_url(url)
        .map(|a| host_of(a).to_ascii_lowercase())
        .unwrap_or_default()
}

/// Authority (`host[:port]`, IPv6 in brackets) of a URL or CONNECT target.
///
/// Returns `None` for origin-form request targets (`/path`), which carry no
/// host. Absolute-form URIs (`http://host/path`) yield their authority; bare
/// `host:port` CONNECT targets are returned as-is.
pub fn authority_from_target(target: &str) -> Option<&str> {
    let target = target.trim();
    if target.is_empty() || target.starts_with('/') {
        return None;
    }
    if let Some((_, rest)) = target.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let authority = strip_userinfo(authority);
        return Some(authority).filter(|a| !a.is_empty());
    }
    Some(target)
}

/// Authority of a string that may or may not carry a scheme.
fn authority_of_url(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let rest = match url.split_once("://") {
        Some((_, r)) => r,
        // No scheme: treat the input as `authority[/path]` so `host:8080/x`
        // and `example.com` both work.
        None => url,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = strip_userinfo(authority);
    Some(authority).filter(|a| !a.is_empty())
}

/// Drop `user:pass@` (or a bare `user@`) from the front of an authority.
fn strip_userinfo(authority: &str) -> &str {
    // rsplit: `user@host` and `user:pass@host` both end at the last `@`;
    // a bare `@` in a hostname is illegal anyway.
    match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_authority_forms() {
        assert_eq!(split_authority("example.com"), ("example.com", None));
        assert_eq!(
            split_authority("example.com:443"),
            ("example.com", Some("443"))
        );
        assert_eq!(
            split_authority("127.0.0.1:8080"),
            ("127.0.0.1", Some("8080"))
        );
        assert_eq!(split_authority("[::1]"), ("::1", None));
        assert_eq!(split_authority("[::1]:443"), ("::1", Some("443")));
        assert_eq!(
            split_authority("[2001:db8::1]:8443"),
            ("2001:db8::1", Some("8443"))
        );
        // Trailing colon is not a port.
        assert_eq!(split_authority("example.com:"), ("example.com:", None));
        // Bare IPv6: the whole literal is the host, no port.
        assert_eq!(split_authority("::1"), ("::1", None));
    }

    #[test]
    fn split_authority_strips_userinfo() {
        assert_eq!(
            split_authority("user:pass@example.com:80"),
            ("example.com", Some("80"))
        );
        assert_eq!(split_authority("user@example.com"), ("example.com", None));
    }

    #[test]
    fn host_of_matches_wire_expectations() {
        assert_eq!(host_of("[::1]:443"), "::1");
        assert_eq!(host_of("example.com:8443"), "example.com");
        assert_eq!(host_of("proxy.lan:8080"), "proxy.lan");
    }

    #[test]
    fn split_host_port_requires_port() {
        assert_eq!(
            split_host_port("example.com:443").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            split_host_port("127.0.0.1:80").unwrap(),
            ("127.0.0.1".into(), 80)
        );
        assert_eq!(split_host_port("[::1]:8080").unwrap(), ("::1".into(), 8080));
        assert!(split_host_port("example.com").is_err());
        assert!(split_host_port(":80").is_err());
        assert!(split_host_port("example.com:notaport").is_err());
        // Bare IPv6 is ambiguous with host:port and must be rejected.
        assert!(split_host_port("::1").is_err());
        assert!(split_host_port("2001:db8::1").is_err());
    }

    #[test]
    fn split_host_port_or_defaults() {
        assert_eq!(
            split_host_port_or("example.com", 443).unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            split_host_port_or("example.com:8443", 443).unwrap(),
            ("example.com".into(), 8443)
        );
        assert_eq!(
            split_host_port_or("[::1]", 8080).unwrap(),
            ("::1".into(), 8080)
        );
        assert_eq!(
            split_host_port_or("example.com", 80).unwrap(),
            ("example.com".into(), 80)
        );
    }

    #[test]
    fn host_from_url_strips_everything_but_host() {
        assert_eq!(host_from_url("https://example.com/a?b=1"), "example.com");
        assert_eq!(
            host_from_url("http://User@WWW.Example.com:443/x"),
            "www.example.com"
        );
        assert_eq!(host_from_url("https://[::1]/"), "::1");
        assert_eq!(host_from_url("example.com:8080/x"), "example.com");
        assert_eq!(host_from_url("bank.example"), "bank.example");
    }

    #[test]
    fn authority_from_target_handles_forms() {
        assert_eq!(
            authority_from_target("http://example.com:8080/a"),
            Some("example.com:8080")
        );
        assert_eq!(
            authority_from_target("example.com:443"),
            Some("example.com:443")
        );
        assert_eq!(authority_from_target("/relative/path"), None);
        assert_eq!(authority_from_target(""), None);
    }
}

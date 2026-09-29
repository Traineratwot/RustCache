//! Query-string redaction for the request log.
//!
//! The traffic history is persisted to `logs.db` and rendered verbatim in the
//! web UI, so credentials that origins accept in the query string (signed CDN
//! URLs, `?api_key=`, OAuth `?access_token=`) would otherwise be stored in
//! clear text. Only the *values* are replaced — the parameter names stay
//! visible so the log is still useful for debugging.

/// Query parameter names whose values are replaced with [`REDACTED`].
///
/// Matched case-insensitively against the exact parameter name.
const SENSITIVE_PARAMS: [&str; 16] = [
    "access_token",
    "api_key",
    "apikey",
    "auth",
    "authorization",
    "id_token",
    "key",
    "passwd",
    "password",
    "pwd",
    "refresh_token",
    "secret",
    "sig",
    "signature",
    "token",
    "x-amz-credential",
];

/// Placeholder written in place of a sensitive value.
pub const REDACTED: &str = "REDACTED";

fn is_sensitive(name: &str) -> bool {
    SENSITIVE_PARAMS
        .iter()
        .any(|p| name.eq_ignore_ascii_case(p))
}

/// Replace the values of well-known credential parameters in `url`'s query string.
///
/// Returns `url` unchanged when it carries no query string or nothing matched.
pub fn redact_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    // Keep a trailing fragment out of the last parameter's value.
    let (query, fragment) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };

    let mut out = String::with_capacity(url.len());
    out.push_str(base);
    out.push('?');
    for (i, pair) in query.split('&').enumerate() {
        if i > 0 {
            out.push('&');
        }
        match pair.split_once('=') {
            Some((name, _)) if is_sensitive(name) => {
                out.push_str(name);
                out.push('=');
                out.push_str(REDACTED);
            }
            _ => out.push_str(pair),
        }
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_credential_params() {
        assert_eq!(
            redact_url("https://x.test/a?api_key=abc123&page=2"),
            "https://x.test/a?api_key=REDACTED&page=2"
        );
        assert_eq!(
            redact_url("https://x.test/a?Token=abc&Signature=def"),
            "https://x.test/a?Token=REDACTED&Signature=REDACTED"
        );
    }

    #[test]
    fn leaves_ordinary_urls_untouched() {
        assert_eq!(redact_url("https://x.test/a/b"), "https://x.test/a/b");
        assert_eq!(
            redact_url("https://x.test/a?page=2&sort=asc"),
            "https://x.test/a?page=2&sort=asc"
        );
    }

    #[test]
    fn keeps_valueless_and_partial_params() {
        assert_eq!(
            redact_url("https://x.test/a?flag&token=t"),
            "https://x.test/a?flag&token=REDACTED"
        );
    }

    #[test]
    fn does_not_swallow_the_fragment() {
        assert_eq!(
            redact_url("https://x.test/a?token=t#frag"),
            "https://x.test/a?token=REDACTED#frag"
        );
    }

    #[test]
    fn substring_matches_are_not_redacted() {
        assert_eq!(
            redact_url("https://x.test/a?monkey=1&keyboard=2"),
            "https://x.test/a?monkey=1&keyboard=2"
        );
    }
}

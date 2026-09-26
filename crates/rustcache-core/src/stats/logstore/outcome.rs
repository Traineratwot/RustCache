//! Cache outcome labels for the request log.
//!
//! The serde names match the historical `TEXT` values stored in
//! `requests.outcome` and returned by the JSON API. Do not rename them.

use serde::{Deserialize, Serialize};

/// Result of serving (or refusing) a request, recorded per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// Fresh entry served from mem/disk without contacting the origin.
    #[serde(rename = "HIT")]
    Hit,
    /// Stale entry revalidated with the origin (304) and served from cache.
    #[serde(rename = "HIT_REVALIDATED")]
    HitRevalidated,
    /// Stale entry served immediately; origin revalidation runs in the
    /// background (optimistic / stale-while-revalidate).
    #[serde(rename = "HIT_STALE")]
    HitStale,
    /// Revalidation returned a new body that replaced the stored entry.
    #[serde(rename = "REVALIDATED")]
    Revalidated,
    /// Cache miss — body fetched from origin (and possibly stored).
    #[serde(rename = "MISS")]
    Miss,
    /// Not cached (auth, exclusion, method, policy) — tunneled or passed through.
    #[serde(rename = "BYPASS")]
    Bypass,
    /// CONNECT / SOCKS5 tunnel bytes.
    #[serde(rename = "TUNNEL")]
    Tunnel,
    /// Upstream or parse failure.
    #[serde(rename = "ERROR")]
    Error,
    /// Unsupported SOCKS5 command (UDP/BIND).
    #[serde(rename = "REJECT_CMD")]
    RejectCmd,
}

impl Outcome {
    /// Every canonical outcome, in stable display order (always present in
    /// `by_outcome` stats, zero-filled).
    pub const ALL: [Outcome; 9] = [
        Outcome::Hit,
        Outcome::HitRevalidated,
        Outcome::HitStale,
        Outcome::Revalidated,
        Outcome::Miss,
        Outcome::Bypass,
        Outcome::Tunnel,
        Outcome::Error,
        Outcome::RejectCmd,
    ];

    /// Wire / DB / JSON representation (historical uppercase labels).
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Hit => "HIT",
            Outcome::HitRevalidated => "HIT_REVALIDATED",
            Outcome::HitStale => "HIT_STALE",
            Outcome::Revalidated => "REVALIDATED",
            Outcome::Miss => "MISS",
            Outcome::Bypass => "BYPASS",
            Outcome::Tunnel => "TUNNEL",
            Outcome::Error => "ERROR",
            Outcome::RejectCmd => "REJECT_CMD",
        }
    }

    /// True for outcomes that mean the response came from cache.
    pub fn is_hit(self) -> bool {
        matches!(
            self,
            Outcome::Hit | Outcome::HitRevalidated | Outcome::HitStale
        )
    }

    /// True for outcomes that count against cache miss rate.
    pub fn is_miss_like(self) -> bool {
        matches!(self, Outcome::Miss | Outcome::Revalidated)
    }

    /// Parse a DB / query-string value. Unknown labels map to `Error` so old
    /// rows keep rendering; new writes only ever store canonical labels.
    pub fn from_db_lossy(s: &str) -> Outcome {
        match s {
            "HIT" => Outcome::Hit,
            "HIT_REVALIDATED" => Outcome::HitRevalidated,
            "HIT_STALE" => Outcome::HitStale,
            "REVALIDATED" => Outcome::Revalidated,
            "MISS" => Outcome::Miss,
            "BYPASS" => Outcome::Bypass,
            "TUNNEL" => Outcome::Tunnel,
            "REJECT_CMD" => Outcome::RejectCmd,
            _ => Outcome::Error,
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_matches_legacy_strings() {
        assert_eq!(serde_json::to_string(&Outcome::Hit).unwrap(), "\"HIT\"");
        assert_eq!(
            serde_json::to_string(&Outcome::HitRevalidated).unwrap(),
            "\"HIT_REVALIDATED\""
        );
        assert_eq!(
            serde_json::to_string(&Outcome::HitStale).unwrap(),
            "\"HIT_STALE\""
        );
        assert_eq!(
            serde_json::from_str::<Outcome>("\"MISS\"").unwrap(),
            Outcome::Miss
        );
    }

    #[test]
    fn all_covers_canonical_labels() {
        let labels: Vec<&str> = Outcome::ALL.iter().map(|o| o.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "HIT",
                "HIT_REVALIDATED",
                "HIT_STALE",
                "REVALIDATED",
                "MISS",
                "BYPASS",
                "TUNNEL",
                "ERROR",
                "REJECT_CMD"
            ]
        );
    }

    #[test]
    fn from_db_lossy_is_defensive() {
        assert_eq!(Outcome::from_db_lossy("HIT"), Outcome::Hit);
        assert_eq!(Outcome::from_db_lossy("weird"), Outcome::Error);
    }
}

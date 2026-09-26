//! `/api/exclusions` — domain/CIDR exclusion list management.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::api::state::ApiState;
use crate::config::persist::write_config_toml;

#[derive(Deserialize)]
pub struct ExclusionBody {
    domain: Option<String>,
    cidr: Option<String>,
}

/// Current exclusion matchers (domains and CIDRs).
pub async fn list_exclusions(State(st): State<ApiState>) -> Json<serde_json::Value> {
    let ms = st.exclusions().await;
    Json(serde_json::json!({"exclusions": ms}))
}

/// Append a domain or CIDR exclusion (no-op if already present) and persist.
pub async fn add_exclusion(
    State(st): State<ApiState>,
    Json(body): Json<ExclusionBody>,
) -> Json<serde_json::Value> {
    let cfg = st.config.get().await;
    let mut domains = cfg.exclude.domains.clone();
    let mut cidrs = cfg.exclude.cidrs.clone();
    if let Some(d) = body.domain {
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    if let Some(c) = body.cidr {
        if !cidrs.contains(&c) {
            cidrs.push(c);
        }
    }
    st.set_exclusions(domains.clone(), cidrs.clone()).await;
    // persist to config file if possible
    let mut new_cfg = cfg.as_ref().clone();
    new_cfg.exclude.domains = domains;
    new_cfg.exclude.cidrs = cidrs;
    if let Err(e) = write_config_toml(&st.config_path, &new_cfg) {
        tracing::warn!(error = %e, "config write failed");
    }
    st.config.set(new_cfg).await;
    Json(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
pub struct ExclusionDelete {
    domain: Option<String>,
    cidr: Option<String>,
    #[serde(default)]
    all: bool,
}

/// Remove one domain/CIDR exclusion, or all of them with `{"all": true}`.
pub async fn clear_exclusions(
    State(st): State<ApiState>,
    Json(body): Json<ExclusionDelete>,
) -> Json<serde_json::Value> {
    let cfg = st.config.get().await;
    let mut domains = cfg.exclude.domains.clone();
    let mut cidrs = cfg.exclude.cidrs.clone();
    if body.all {
        domains.clear();
        cidrs.clear();
    } else {
        if let Some(d) = body.domain {
            domains.retain(|x| x != &d);
        }
        if let Some(c) = body.cidr {
            cidrs.retain(|x| x != &c);
        }
    }
    st.set_exclusions(domains.clone(), cidrs.clone()).await;
    let mut new_cfg = cfg.as_ref().clone();
    new_cfg.exclude.domains = domains;
    new_cfg.exclude.cidrs = cidrs;
    if let Err(e) = write_config_toml(&st.config_path, &new_cfg) {
        tracing::warn!(error = %e, "config write failed");
    }
    st.config.set(new_cfg).await;
    Json(serde_json::json!({"ok": true}))
}

//! `/api/stats` — cache/proxy metrics snapshot.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::api::state::ApiState;

/// Serialize the engine metrics snapshot; empty object if serialization fails.
pub async fn stats(State(st): State<ApiState>) -> Json<Value> {
    let snap = st.engine.metrics().snapshot();
    Json(serde_json::to_value(snap).unwrap_or(json!({})))
}

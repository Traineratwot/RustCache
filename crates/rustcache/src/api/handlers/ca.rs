//! `/api/ca.crt` — root CA download.

use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::api::state::ApiState;

/// Serve the root CA PEM as a downloadable `ca.crt` attachment.
pub async fn ca_crt(State(st): State<ApiState>) -> Response {
    let mut resp = (
        [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
        st.ca.cert_pem.clone(),
    )
        .into_response();
    if let Ok(val) = "attachment; filename=\"ca.crt\"".parse() {
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, val);
    }
    resp
}

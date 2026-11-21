//! Credential-free session view for a server-side portal bridge. The caller
//! supplies a real NyxID bearer; this route creates no independent login cookie.
use crate::{AuthError, NyxIdAuth};
use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::sync::Arc;

pub fn router(auth: NyxIdAuth) -> Router {
    Router::new()
        .route("/auth/session", get(read))
        .with_state(Arc::new(auth))
}
async fn read(
    State(auth): State<Arc<NyxIdAuth>>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let principal = auth.authenticate(&headers).await?;
    Ok(([(header::CACHE_CONTROL,"no-store")], Json(serde_json::json!({
        "authenticated":true,"subject":principal.subject,"permissions":principal.permissions,"expires_at":principal.expires_at
    }))).into_response())
}

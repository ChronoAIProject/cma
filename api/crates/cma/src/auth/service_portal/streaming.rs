//! Activity stays a stream through the existing browser credential boundary.
//! The transport's finite lifetime forces normal reauthentication on reconnect.
use super::{credentials::Custody, AuthState, PortalError};
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::{io, sync::Arc, time::Duration};

const STREAM_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("activity stream authorization changed: {0}")]
struct StreamAuthorizationError(#[source] PortalError);

pub(super) async fn response(
    upstream: reqwest::Response,
    mut headers: HeaderMap,
    auth: Arc<AuthState>,
    custody: Custody,
) -> Result<Response, PortalError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next());
    if content_type != Some("text/event-stream") {
        return Err(StatusCode::BAD_GATEWAY.into());
    }
    custody.current(&auth).await?;
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    let stream = futures_util::stream::try_unfold(
        (upstream, auth, custody, 0usize),
        |(mut upstream, auth, custody, bytes)| async move {
            // Gateway heartbeats arrive every 15 seconds. An unresponsive
            // upstream is closed so EventSource can reconnect and resync.
            let chunk = tokio::time::timeout(Duration::from_secs(30), upstream.chunk())
                .await
                .map_err(|_| io::Error::other("activity stream idle"))?
                .map_err(|_| io::Error::other("activity stream interrupted"))?;
            let Some(chunk) = chunk else {
                return Ok(None);
            };
            let bytes = bytes.saturating_add(chunk.len());
            if bytes > STREAM_BYTES {
                return Err(io::Error::other("activity stream reconnect required"));
            }
            // Never release a newly arrived frame after logout, token rotation
            // or a changed owner authority. Reconnect resolves fresh custody.
            custody
                .current(&auth)
                .await
                .map_err(|error| io::Error::other(StreamAuthorizationError(error)))?;
            Ok(Some((chunk, (upstream, auth, custody, bytes))))
        },
    );
    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

//! Dedicated callback authentication; never browser/session/management auth.
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use futures_util::StreamExt;
use zeroize::Zeroizing;

use super::*;

const MAX_BODY: usize = 1_048_576;

struct CallbackHttp {
    ingress: Option<Arc<CallbackIngress>>,
    readers: Semaphore,
}

pub(crate) fn router(ingress: Option<Arc<CallbackIngress>>) -> Router {
    Router::new()
        .route("/api/v1/bridge-callbacks/{bridge_id}", post(callback))
        .with_state(Arc::new(CallbackHttp {
            ingress,
            readers: Semaphore::new(16),
        }))
}

async fn callback(
    State(state): State<Arc<CallbackHttp>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let result = accept(state, id, request).await;
    let mut response = match result {
        Ok(receipt) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status":"inbox_persisted", "receipt":receipt,
            })),
        )
            .into_response(),
        Err(error) => error.response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn accept(
    state: Arc<CallbackHttp>,
    id: String,
    request: Request,
) -> Result<InboxReceipt, HttpFailure> {
    let id = id
        .strip_prefix("brg_")
        .filter(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 252
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(|_| BridgeId(id.clone()))
        .ok_or(HttpFailure::Core(CallbackError::BindingUnavailable))?;
    let (parts, body) = request.into_parts();
    validate_headers(&parts.headers)?;
    let ingress = state.ingress.as_ref().ok_or(HttpFailure::NotConfigured)?;
    let _reader = state
        .readers
        .try_acquire()
        .map_err(|_| CallbackError::Capacity)?;
    let bytes = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Zeroizing::new(Vec::new());
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CallbackError::Payload)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
                return Err(CallbackError::Bounds);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| HttpFailure::BodyTimeout)??;
    ingress
        .accept(&id, &parts.headers, &bytes)
        .await
        .map_err(Into::into)
}

fn validate_headers(headers: &HeaderMap) -> Result<(), HttpFailure> {
    if super::header(headers, "content-type", true, 64)? != Some("application/json") {
        return Err(HttpFailure::ContentType);
    }
    super::parse_headers(headers)?;
    if let Some(length) = super::header(headers, "content-length", false, 20)? {
        let length: usize = length.parse().map_err(|_| CallbackError::Payload)?;
        if length > MAX_BODY {
            return Err(CallbackError::Bounds.into());
        }
    }
    Ok(())
}

enum HttpFailure {
    Core(CallbackError),
    NotConfigured,
    ContentType,
    BodyTimeout,
}

impl From<CallbackError> for HttpFailure {
    fn from(value: CallbackError) -> Self {
        Self::Core(value)
    }
}

impl HttpFailure {
    fn response(self) -> Response {
        use CallbackError as E;
        let (status, code, acceptance) = match self {
            Self::Core(E::Authentication) => (
                StatusCode::UNAUTHORIZED,
                "callback_authentication_failed",
                "not_accepted",
            ),
            Self::Core(E::Bounds) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "callback_bounds_exceeded",
                "not_accepted",
            ),
            Self::Core(E::Payload) => (
                StatusCode::BAD_REQUEST,
                "callback_payload_invalid",
                "not_accepted",
            ),
            Self::Core(E::ReplayConflict) => (
                StatusCode::CONFLICT,
                "callback_replay_conflict",
                "not_accepted",
            ),
            Self::Core(E::Capacity) => (
                StatusCode::TOO_MANY_REQUESTS,
                "callback_capacity_exceeded",
                "not_accepted",
            ),
            Self::Core(E::KeysUnavailable) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "callback_keys_unavailable",
                "not_accepted",
            ),
            Self::Core(E::BindingUnavailable) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "callback_binding_unavailable",
                "not_accepted",
            ),
            Self::Core(E::Unavailable) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "callback_acceptance_unknown",
                "unknown",
            ),
            Self::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                "callback_ingress_unavailable",
                "not_accepted",
            ),
            Self::ContentType => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "callback_json_required",
                "not_accepted",
            ),
            Self::BodyTimeout => (
                StatusCode::REQUEST_TIMEOUT,
                "callback_body_timeout",
                "not_accepted",
            ),
        };
        let mut response = (
            status,
            Json(serde_json::json!({"code":code,"acceptance":acceptance})),
        )
            .into_response();
        if matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, "1".parse().unwrap());
        }
        response
    }
}

#[cfg(test)]
mod tests;

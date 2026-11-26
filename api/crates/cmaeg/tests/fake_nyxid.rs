use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::CONTENT_LENGTH, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use bytes::Bytes;
use futures_util::StreamExt;
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot};

#[path = "fake_nyxid_console.rs"]
pub mod console;

pub struct FakeNyxid {
    pub base_url: String,
    pub service_tokens: Arc<TokenState>,
    pub attachments: Arc<AttachmentState>,
    /// Console sign-in: discovery, tokens, revocation and organizations.
    pub oauth: Arc<console::OAuthState>,
    shutdown: Option<oneshot::Sender<()>>,
}

/// Per-test behaviour of the relay attachment route. A request bearing
/// `reply_token` is answered with `reply_status`, one bearing `channel_key`
/// with `key_status`, anything else with 401.
pub struct AttachmentState {
    pub reply_token: Mutex<String>,
    pub channel_key: Mutex<String>,
    pub reply_status: AtomicU16,
    pub key_status: AtomicU16,
    pub body: Mutex<Bytes>,
    /// Sent before the body, so a short client deadline expires mid-body.
    pub delay_ms: AtomicU64,
    /// A `Content-Length` to declare instead of the body's own; the declared
    /// bytes may never arrive.
    pub declared_length: Mutex<Option<u64>>,
    /// Stream the body in chunks without any `Content-Length`.
    pub chunked: AtomicBool,
    pub requests: Mutex<Vec<AttachmentRequest>>,
}

/// What one attachment request named and which bearer it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRequest {
    pub message_id: String,
    pub index: String,
    pub bearer: String,
}

impl Default for AttachmentState {
    fn default() -> Self {
        Self {
            reply_token: Mutex::new("relay-reply-fixture".into()),
            channel_key: Mutex::new("relay-channel-fixture".into()),
            reply_status: AtomicU16::new(200),
            key_status: AtomicU16::new(200),
            body: Mutex::new(Bytes::from_static(b"attachment-bytes")),
            delay_ms: AtomicU64::new(0),
            declared_length: Mutex::new(None),
            chunked: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
        }
    }
}

pub struct TokenState {
    issuer: String,
    pub calls: AtomicUsize,
    pub expires_in: AtomicU64,
    /// `/oauth/token` and `users/me` serve console grants and tokens too.
    pub oauth: Arc<console::OAuthState>,
}

impl FakeNyxid {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, signal) = oneshot::channel();
        let oauth = Arc::new(console::OAuthState::default());
        *oauth.base_url.lock().unwrap() = format!("http://{address}");
        let service_tokens = Arc::new(TokenState {
            issuer: format!("http://{address}"),
            calls: AtomicUsize::new(0),
            expires_in: AtomicU64::new(3600),
            oauth: oauth.clone(),
        });
        let attachments = Arc::new(AttachmentState::default());
        let router = Router::new()
            .route("/api/v1/users/me", get(users_me))
            .route("/oauth/token", post(service_token))
            .route("/oauth/introspect", post(introspect))
            .with_state(service_tokens.clone())
            .merge(
                Router::new()
                    .route(
                        "/api/v1/channel-relay/messages/{message_id}/attachments/{index}",
                        get(attachment),
                    )
                    .with_state(attachments.clone()),
            )
            .merge(
                Router::new()
                    .route("/.well-known/openid-configuration", get(console::discovery))
                    .route("/oauth/authorize", get(console::authorize))
                    .route("/oauth/revoke", post(console::revoke))
                    .route("/api/v1/orgs", get(console::org_list))
                    .route(
                        "/api/v1/orgs/{id}/authorization",
                        get(console::org_authorization),
                    )
                    .with_state(oauth.clone()),
            );
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            service_tokens,
            attachments,
            oauth,
            shutdown: Some(shutdown),
        }
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn users_me(State(state): State<Arc<TokenState>>, headers: HeaderMap) -> Response {
    if let Some(response) = console::users_me(&state.oauth, &headers).await {
        return response;
    }
    let Some(value) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response();
    };
    match value {
        "Bearer token-alice" => Json(serde_json::json!({"subject":"alice"})).into_response(),
        "Bearer token-bob" => Json(serde_json::json!({"subject":"bob"})).into_response(),
        "Bearer token-org-a" => {
            Json(serde_json::json!({"subject":"org-a","user_type":"org"})).into_response()
        }
        _ => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response(),
    }
}

async fn service_token(
    State(state): State<Arc<TokenState>>,
    headers: HeaderMap,
    Form(form): Form<std::collections::BTreeMap<String, String>>,
) -> Response {
    if form.get("grant_type").map(String::as_str) != Some("client_credentials") {
        return console::token(state.oauth.clone(), headers, form).await;
    }
    assert_eq!(form["client_id"], "fixture-client");
    assert_eq!(form["client_secret"], "fixture-client-secret");
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["scope"], "openid roles");
    state.calls.fetch_add(1, Ordering::SeqCst);
    Json(
        serde_json::json!({"access_token":"cma-service", "token_type":"Bearer",
        "expires_in":state.expires_in.load(Ordering::SeqCst)}),
    )
    .into_response()
}

async fn attachment(
    State(state): State<Arc<AttachmentState>>,
    Path((message_id, index)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    state.requests.lock().unwrap().push(AttachmentRequest {
        message_id,
        index,
        bearer: bearer.clone(),
    });
    let status = if bearer == *state.reply_token.lock().unwrap() {
        state.reply_status.load(Ordering::SeqCst)
    } else if bearer == *state.channel_key.lock().unwrap() {
        state.key_status.load(Ordering::SeqCst)
    } else {
        401
    };
    let status = StatusCode::from_u16(status).unwrap();
    if status != StatusCode::OK {
        return (status, Json(serde_json::json!({"error": "refused"}))).into_response();
    }
    let body = state.body.lock().unwrap().clone();
    let delay = Duration::from_millis(state.delay_ms.load(Ordering::SeqCst));
    let declared = *state.declared_length.lock().unwrap();
    let chunked = state.chunked.load(Ordering::SeqCst);
    if declared.is_none() && !chunked && delay.is_zero() {
        return (status, body).into_response();
    }
    let pieces: Vec<Bytes> = body.chunks(16 * 1024).map(Bytes::copy_from_slice).collect();
    let stream = futures_util::stream::once(tokio::time::sleep(delay))
        .flat_map(move |_| futures_util::stream::iter(pieces.clone()))
        .map(Ok::<_, Infallible>);
    let mut response = Response::builder().status(status);
    if let Some(length) = declared {
        response = response.header(CONTENT_LENGTH, length);
    } else if !chunked {
        response = response.header(CONTENT_LENGTH, body.len());
    }
    response.body(Body::from_stream(stream)).unwrap()
}

async fn introspect(
    State(state): State<Arc<TokenState>>,
    Form(form): Form<std::collections::BTreeMap<String, String>>,
) -> impl IntoResponse {
    assert_eq!(form["client_id"], "gateway-management");
    assert_eq!(form["client_secret"], "gateway-management-fixture");
    let subject = match form.get("token").map(String::as_str) {
        Some("token-alice" | "token-alice-manager" | "token-alice-create-only") => {
            "alice".to_owned()
        }
        Some("token-bob") => "bob".to_owned(),
        Some(token) if state.oauth.subject_of(token).is_some() => {
            state.oauth.subject_of(token).unwrap()
        }
        _ => return Json(serde_json::json!({"active":false})),
    };
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    Json(
        serde_json::json!({"active":true,"sub":subject,"iss":state.issuer,"exp":expiry,
        "token_type":"access","resource":[format!("{}/api/v1/proxy/s/cma-event-gateway",state.issuer)],
        "permissions":if form.get("token").map(String::as_str) == Some("token-alice-manager") {
            vec!["cma-eg:channels:read", "cma-eg:channels:create", "cma-eg:channels:update"]
        } else if form.get("token").map(String::as_str) == Some("token-alice-create-only") {
            vec!["cma-eg:channels:create"]
        } else { vec!["cma-eg:admin"] }}),
    )
}

//! Synthetic provider discovery/proxy; ownership is enforced by the real child.
use axum::{
    body::Bytes,
    extract::{Path, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::{HostedControlTransport, ProviderHostedBackend, ProviderHostedControlTransport},
    provider::SandboxClient,
    DesktopOwnership, RuntimeHandle, SandboxAdapter,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

mod relay;

pub(super) struct StateData {
    address: std::net::SocketAddr,
    receiver: RwLock<std::net::SocketAddr>,
    provider_token: String,
    runtime_token: String,
    runtime_id: String,
    http: reqwest::Client,
    pub(super) confirmations: Mutex<Vec<DesktopOwnership>>,
    pub(super) opens: AtomicUsize,
    pub(super) active: AtomicUsize,
    pub(super) lose_confirm_ack: AtomicBool,
    pub(super) lose_revoke_ack: AtomicBool,
    pub(super) hold_renew_ack: AtomicBool,
    pub(super) renewal_entered: tokio::sync::Notify,
    pub(super) renewal_release: tokio::sync::Notify,
    pub(super) stop: tokio::sync::watch::Sender<bool>,
}

pub(super) struct ProviderProxy {
    pub(super) state: Arc<StateData>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for ProviderProxy {
    fn drop(&mut self) {
        self.state.stop.send_replace(true);
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

impl ProviderProxy {
    pub(super) async fn start(receiver: std::net::SocketAddr, runtime: &str, token: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, _) = tokio::sync::watch::channel(false);
        let state = Arc::new(StateData {
            address,
            receiver: RwLock::new(receiver),
            provider_token: uuid::Uuid::new_v4().to_string(),
            runtime_token: token.into(),
            runtime_id: runtime.into(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            confirmations: Mutex::new(Vec::new()),
            opens: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            lose_confirm_ack: AtomicBool::new(false),
            lose_revoke_ack: AtomicBool::new(false),
            hold_renew_ack: AtomicBool::new(false),
            renewal_entered: tokio::sync::Notify::new(),
            renewal_release: tokio::sync::Notify::new(),
            stop,
        });
        let app = Router::new()
            .route("/v1/sandboxes/{runtime}/endpoints/9090", get(discovery))
            .route("/proxy/runtime/desktop/ownership/{action}", post(control))
            .route("/proxy/runtime/desktop", get(desktop))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            state,
            server: Some(server),
        }
    }

    pub(super) async fn adapter(&self, sandbox: &str) -> Arc<dyn SandboxAdapter> {
        // Each call creates an independent API-side adapter/transport/HTTP pool.
        let client = SandboxClient::new(
            format!("http://{}", self.state.address),
            self.state.provider_token.clone(),
        );
        let transport =
            ProviderHostedControlTransport::for_retained_opensandbox(client.clone()).unwrap();
        transport
            .bind_runtime(&RuntimeHandle::new(self.state.runtime_id.clone()), sandbox)
            .await
            .unwrap();
        let backend =
            ProviderHostedBackend::new(client, Arc::new(transport), "desktop-fixture").unwrap();
        Arc::new(CodexAdapter::with_backend(
            backend,
            CodexSandboxSpec::default(),
        ))
    }

    pub(super) fn receiver(&self, address: std::net::SocketAddr) {
        assert!(address.ip().is_loopback());
        *self.state.receiver.write().unwrap() = address;
    }

    pub(super) fn last_owner(&self) -> DesktopOwnership {
        self.state
            .confirmations
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone()
    }

    pub(super) async fn stop(mut self) {
        let server = self.server.take().unwrap();
        server.abort();
        let stopped = server.await;
        assert!(stopped.is_ok() || stopped.unwrap_err().is_cancelled());
        self.state.stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.active.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

fn authorized(state: &StateData, headers: &HeaderMap) -> bool {
    headers
        .get("open-sandbox-api-key")
        .and_then(|header| header.to_str().ok())
        == Some(state.provider_token.as_str())
}

async fn discovery(
    State(state): State<Arc<StateData>>,
    Path(runtime): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if runtime != state.runtime_id {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(serde_json::json!({"endpoint": format!("{}/proxy", state.address)})).into_response()
}

async fn control(
    State(state): State<Arc<StateData>>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !["challenge", "confirm", "revoke"].contains(&action.as_str()) || body.len() > 8192 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let address = *state.receiver.read().unwrap();
    let result = state
        .http
        .post(format!(
            "http://{address}/runtime/desktop/ownership/{action}"
        ))
        .header("x-cma-runtime-token", &state.runtime_token)
        .header("content-type", "application/json")
        .body(body.to_vec())
        .send()
        .await;
    let Ok(response) = result else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let status = response.status();
    let Ok(bytes) = response.bytes().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    if status.is_success() && action == "confirm" {
        let confirmation: layer3_adapter::desktop_ownership::wire::DesktopConfirmation =
            serde_json::from_slice(&body).unwrap();
        let renewal = {
            let mut confirmations = state.confirmations.lock().unwrap();
            let renewal = confirmations.last().is_some_and(|previous| {
                previous.session_id == confirmation.ownership.session_id
                    && previous.epoch == confirmation.ownership.epoch
            });
            confirmations.push(confirmation.ownership);
            renewal
        };
        if renewal && state.hold_renew_ack.load(Ordering::SeqCst) {
            state.renewal_entered.notify_one();
            state.renewal_release.notified().await;
        }
        if state.lose_confirm_ack.load(Ordering::SeqCst) {
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }
    if status.is_success() && action == "revoke" && state.lose_revoke_ack.load(Ordering::SeqCst) {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    (status, [("content-type", "application/json")], bytes).into_response()
}

async fn desktop(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let address = *state.receiver.read().unwrap();
    let mut request = format!("ws://{address}/runtime/desktop")
        .into_client_request()
        .unwrap();
    let mut token: axum::http::HeaderValue = state.runtime_token.parse().unwrap();
    token.set_sensitive(true);
    request.headers_mut().insert("x-cma-runtime-token", token);
    let owner_header = layer3_adapter::desktop_ownership::wire::DESKTOP_OWNER_HEADER;
    if let Some(owner) = headers.get(owner_header) {
        request.headers_mut().insert(owner_header, owner.clone());
    }
    let Ok(Ok((upstream, _))) = tokio::time::timeout(
        Duration::from_secs(3),
        tokio_tungstenite::connect_async(request),
    )
    .await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    state.opens.fetch_add(1, Ordering::SeqCst);
    state.active.fetch_add(1, Ordering::SeqCst);
    let active = relay::Active(state.clone());
    ws.on_upgrade(move |socket| relay::run(socket, upstream, active))
}

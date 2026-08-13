//! chrono-bot sandboxed agent runtime.
//!
//! Runs in the bot pod, one per session. Supervises the agent kernel under a
//! real pseudo-terminal and relays that terminal to the control plane over a
//! WebSocket. This binary is never linked into the control plane.

mod agent_config;
mod conversation;
mod pty;
mod upstream;

use std::net::SocketAddr;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::agent_config::{AgentConfig, PLACEHOLDER_KEY};
use crate::pty::{PtyRegistry, PtySession};
use crate::upstream::{CredentialHolder, UpstreamState};

#[derive(Clone)]
struct AppState {
    registry: PtyRegistry,
    command: String,
    /// The owner's credential, swappable while the agent runs.
    credential: CredentialHolder,
    /// This pod's home directory, where the agent keeps its conversation.
    home: std::path::PathBuf,
    /// Set once a conversation has been handed to this pod.
    ///
    /// The agent is not started until something attaches, which is what makes
    /// a handover possible over HTTP at all: the conversation can arrive after
    /// the pod is running and still be there before the agent reads it.
    resuming: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    /// The command to start the agent with, given what this pod has been told.
    fn agent_command(&self) -> String {
        if !self.resuming.load(std::sync::atomic::Ordering::SeqCst) {
            return self.command.clone();
        }
        // Configuration rather than a constant: which flag continues a
        // conversation is the agent's decision, it differs between kernels,
        // and it changes between agent versions. Getting it wrong must be
        // correctable without shipping a new binary.
        let resume = std::env::var("CHRONO_RUNTIME_RESUME_ARGS")
            .unwrap_or_else(|_| "resume --last".to_string());
        format!("{} {resume}", self.command)
    }
}

/// Geometry the control plane requests when it attaches.
#[derive(Debug, Deserialize)]
struct AttachParams {
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
}

/// A control message from the control plane. Terminal bytes travel as binary
/// frames; this is the text-frame vocabulary, mirrored on the browser side.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ControlFrame {
    Resize {
        cols: u16,
        rows: u16,
    },
    Signal {
        sig: String,
    },
    /// Replace the credential the local proxy presents upstream.
    ///
    /// Arrives on the same socket as terminal traffic so a refresh needs no
    /// second connection, and takes effect without restarting the agent.
    Credential {
        value: String,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind: SocketAddr = std::env::var("CHRONO_RUNTIME_BIND")
        .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
        .parse()
        .expect("CHRONO_RUNTIME_BIND must be a socket address");

    let home: std::path::PathBuf = std::env::var("HOME")
        .unwrap_or_else(|_| "/home/chrono".to_string())
        .into();

    // Which agent to supervise. Configurable so the same binary can host any
    // kernel, and so a shell can stand in when no kernel is installed.
    let command =
        std::env::var("CHRONO_RUNTIME_COMMAND").unwrap_or_else(|_| "/bin/bash -l".to_string());

    let credential = CredentialHolder::new();

    // The agent talks to this proxy, not to the model endpoint. Configuration
    // is written before anything starts so the first turn already goes through
    // it, and a credential that arrives later needs no restart.
    let proxy_bind: std::net::SocketAddr = std::env::var("CHRONO_UPSTREAM_BIND")
        .unwrap_or_else(|_| "127.0.0.1:9091".to_string())
        .parse()
        .expect("CHRONO_UPSTREAM_BIND must be a socket address");
    let upstream_base = std::env::var("CHRONO_UPSTREAM_BASE_URL").unwrap_or_default();

    if upstream_base.trim().is_empty() {
        // Without an upstream the agent can still run interactively; it simply
        // has nothing to call. Saying so beats a confusing failure mid-turn.
        tracing::warn!(
            "CHRONO_UPSTREAM_BASE_URL is unset; the agent will start but has no model endpoint"
        );
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/chrono".to_string());
        let agent_config = AgentConfig {
            // No version segment here. The upstream is a brokered service whose own
            // base URL already carries one, and the broker appends whatever path
            // arrives; adding /v1 produces .../v1/v1/responses and a 404.
            proxy_base_url: format!("http://{proxy_bind}"),
            model: std::env::var("CHRONO_UPSTREAM_MODEL")
                .unwrap_or_else(|_| "gpt-5.6-sol".to_string()),
            wire_api: std::env::var("CHRONO_UPSTREAM_WIRE_API")
                .unwrap_or_else(|_| "responses".to_string()),
            provider_name: "Chrono LLM".to_string(),
            // Defaults here match the core's own: a runtime started without
            // these must not be more permissive than one configured
            // explicitly.
            effort: std::env::var("CHRONO_AGENT_EFFORT").unwrap_or_else(|_| "medium".to_string()),
            approval_policy: std::env::var("CHRONO_AGENT_APPROVAL")
                .unwrap_or_else(|_| "on-request".to_string()),
            sandbox_mode: std::env::var("CHRONO_AGENT_SANDBOX")
                .unwrap_or_else(|_| "read-only".to_string()),
        };
        if let Err(e) = agent_config.write(std::path::Path::new(&home)) {
            tracing::error!(error = %e, "cannot write agent configuration");
            std::process::exit(2);
        }

        spawn_credential_proxy(
            "model",
            proxy_bind,
            upstream_base.clone(),
            credential.clone(),
        );
    }

    // The agent also drives a CLI that acts on the owner's behalf against the
    // identity platform itself. It gets its own listener onto the platform root,
    // sharing one credential so a single renewal updates both.
    let platform_upstream = std::env::var("CHRONO_PLATFORM_BASE_URL").unwrap_or_default();
    if !platform_upstream.trim().is_empty() {
        let platform_bind: std::net::SocketAddr = std::env::var("CHRONO_PLATFORM_PROXY_BIND")
            .unwrap_or_else(|_| "127.0.0.1:9092".to_string())
            .parse()
            .expect("CHRONO_PLATFORM_PROXY_BIND must be a socket address");
        spawn_credential_proxy(
            "platform",
            platform_bind,
            platform_upstream,
            credential.clone(),
        );
    }

    let state = AppState {
        registry: PtyRegistry::new(),
        command: command.clone(),
        credential,
        home: home.clone(),
        resuming: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .route("/runtime/{id}/terminal", get(attach))
        // What makes this pod replaceable. The control plane takes this before
        // tearing the pod down and hands it to the one that takes over.
        .route(
            "/runtime/conversation",
            get(conversation_snapshot).post(conversation_restore),
        )
        .with_state(state);

    tracing::info!(%bind, %command, "runtime supervisor listening");
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("runtime supervisor must bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("runtime supervisor must serve");
}

/// Take a conversation handed over from the runtime this one replaces.
///
/// Refused once the agent is running. Laying a conversation down underneath a
/// live agent would do nothing useful — it reads its history at startup — and
/// accepting the call would report success for a handover that did not happen.
async fn conversation_restore(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if body.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "a conversation to restore was expected",
        )
            .into_response();
    }
    if state.registry.is_running().await {
        return (
            axum::http::StatusCode::CONFLICT,
            "this runtime has already started its agent",
        )
            .into_response();
    }
    match conversation::restore(&state.home, &body, &stamp()) {
        Ok(_) => {
            state
                .resuming
                .store(true, std::sync::atomic::Ordering::SeqCst);
            axum::http::StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            // Loud, because the caller is about to tear down the runtime this
            // conversation came from. Reporting success here would lose it.
            tracing::error!(error = %e, "cannot restore the conversation handed to this runtime");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot restore the conversation: {e}"),
            )
                .into_response()
        }
    }
}

/// A filename-safe stamp, distinct per restore within a pod.
fn stamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    format!("carried-{seconds}")
}

/// Hand over the agent's conversation so it can be resumed elsewhere.
async fn conversation_snapshot(State(state): State<AppState>) -> impl IntoResponse {
    match conversation::capture(&state.home) {
        Ok(Some(bytes)) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        // Nothing to carry is not a failure, and must be distinguishable from
        // one: a caller that read an error as "no conversation" would replace
        // a runtime and silently drop the session's history.
        Ok(None) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "cannot capture the conversation");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot capture the conversation: {e}"),
            )
                .into_response()
        }
    }
}

/// Start a listener that forwards to `upstream` with the owner's credential.
///
/// Several of these run at once, one per upstream the agent talks to, all
/// sharing a single credential holder so one renewal reaches every consumer.
fn spawn_credential_proxy(
    name: &'static str,
    bind: std::net::SocketAddr,
    upstream_base: String,
    credential: CredentialHolder,
) {
    let state = UpstreamState::new(upstream_base.clone(), credential);
    let app = Router::new()
        .fallback(axum::routing::any(upstream::proxy))
        .with_state(state);
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(bind).await {
            Ok(listener) => {
                tracing::info!(proxy = name, %bind, upstream = %upstream_base, "credential proxy listening");
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!(proxy = name, error = %e, "credential proxy stopped");
                }
            }
            Err(e) => {
                tracing::error!(proxy = name, %bind, error = %e, "credential proxy cannot bind")
            }
        }
    });
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let live = state.registry.len().await;
    // Whether a credential is held, never which one: an operator needs to know
    // if calls would fail, not what the value is.
    axum::Json(serde_json::json!({
        "status": "ready",
        "runtimes": live,
        "credential": state.credential.is_set().await,
    }))
}

async fn attach(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    Query(params): Query<AttachParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let cols = params.cols.unwrap_or(80).max(1);
    let rows = params.rows.unwrap_or(24).max(1);
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = relay(socket, state, id.clone(), cols, rows).await {
            tracing::warn!(runtime = %id, error = %e, "terminal relay ended with an error");
        }
    })
}

async fn relay(
    socket: WebSocket,
    state: AppState,
    id: String,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let session: std::sync::Arc<PtySession> = state
        .registry
        .get_or_start(&id, &state.agent_command(), cols, rows, PLACEHOLDER_KEY)
        .await?;

    // Subscribe before the first resize so no output produced during startup
    // is missed by this attachment.
    let mut output = session.output.subscribe();
    session.resize(cols, rows)?;

    let (mut sink, mut source) = socket.split();

    let runtime = id.clone();
    let mut pump_out = tokio::spawn(async move {
        loop {
            match output.recv().await {
                Ok(chunk) => {
                    if sink.send(Message::Binary(chunk.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    // The consumer can always recover from a fresh screen, so
                    // lagging is survivable and must be reported, not hidden.
                    tracing::warn!(runtime = %runtime, skipped, "terminal consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let session_in = std::sync::Arc::clone(&session);
    let credential_in = state.credential.clone();
    let runtime = id.clone();
    let mut pump_in = tokio::spawn(async move {
        while let Some(frame) = source.next().await {
            match frame {
                Ok(Message::Binary(bytes)) => {
                    if let Err(e) = session_in.write_input(&bytes) {
                        tracing::warn!(runtime = %runtime, error = %e, "pty write failed");
                        break;
                    }
                }
                Ok(Message::Text(text)) => match serde_json::from_str::<ControlFrame>(&text) {
                    Ok(ControlFrame::Resize { cols, rows }) => {
                        if let Err(e) = session_in.resize(cols, rows) {
                            tracing::warn!(runtime = %runtime, error = %e, "resize rejected");
                        }
                    }
                    Ok(ControlFrame::Credential { value }) => {
                        // Never logged with its value; the holder reports only
                        // that a replacement happened.
                        credential_in.set(value).await;
                    }
                    Ok(ControlFrame::Signal { sig }) => {
                        // Signals are delivered in band as the control
                        // character the kernel already understands, so no
                        // separate signalling path is needed.
                        let byte: Option<u8> = match sig.as_str() {
                            "INT" => Some(0x03),
                            "QUIT" => Some(0x1c),
                            "TERM" | "HUP" => Some(0x04),
                            _ => None,
                        };
                        match byte {
                            Some(b) => {
                                if let Err(e) = session_in.write_input(&[b]) {
                                    tracing::warn!(runtime = %runtime, error = %e, "signal write failed");
                                }
                            }
                            None => {
                                tracing::warn!(runtime = %runtime, %sig, "unknown signal ignored")
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(runtime = %runtime, error = %e, "malformed control frame ignored")
                    }
                },
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(runtime = %runtime, error = %e, "terminal read failed");
                    break;
                }
            }
        }
    });

    // A dropped connection detaches; it does not end the session. The process
    // keeps running and the next attach reuses it.
    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        _ = &mut pump_in => pump_out.abort(),
    }
    tracing::info!(runtime = %id, "terminal detached");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler installs");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installs")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

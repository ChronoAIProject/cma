//! chrono-bot sandboxed agent runtime.
//!
//! Runs in the bot pod, one per session. Supervises the agent kernel under a
//! real pseudo-terminal and relays that terminal to the control plane over a
//! WebSocket. This binary is never linked into the control plane.

mod agent_config;
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
            proxy_base_url: format!("http://{proxy_bind}/v1"),
            model: std::env::var("CHRONO_UPSTREAM_MODEL")
                .unwrap_or_else(|_| "gpt-5.6-sol".to_string()),
            wire_api: std::env::var("CHRONO_UPSTREAM_WIRE_API")
                .unwrap_or_else(|_| "responses".to_string()),
            provider_name: "Chrono LLM".to_string(),
        };
        if let Err(e) = agent_config.write(std::path::Path::new(&home)) {
            tracing::error!(error = %e, "cannot write agent configuration");
            std::process::exit(2);
        }

        let upstream = UpstreamState::new(upstream_base.clone(), credential.clone());
        let proxy_app = Router::new()
            .fallback(axum::routing::any(upstream::proxy))
            .with_state(upstream);
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(proxy_bind).await {
                Ok(listener) => {
                    tracing::info!(%proxy_bind, upstream = %upstream_base, "upstream proxy listening");
                    if let Err(e) = axum::serve(listener, proxy_app).await {
                        tracing::error!(error = %e, "upstream proxy stopped");
                    }
                }
                Err(e) => tracing::error!(%proxy_bind, error = %e, "upstream proxy cannot bind"),
            }
        });
    }

    let state = AppState {
        registry: PtyRegistry::new(),
        command: command.clone(),
        credential,
    };

    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .route("/runtime/{id}/terminal", get(attach))
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
        .get_or_start(&id, &state.command, cols, rows, PLACEHOLDER_KEY)
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

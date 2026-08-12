//! chrono-bot sandboxed agent runtime.
//!
//! Runs in the bot pod, one per session. Supervises the agent kernel under a
//! real pseudo-terminal and relays that terminal to the control plane over a
//! WebSocket. This binary is never linked into the control plane.

mod pty;

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

use crate::pty::{PtyRegistry, PtySession};

#[derive(Clone)]
struct AppState {
    registry: PtyRegistry,
    command: String,
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
    Resize { cols: u16, rows: u16 },
    Signal { sig: String },
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

    let state = AppState {
        registry: PtyRegistry::new(),
        command: command.clone(),
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
    axum::Json(serde_json::json!({ "status": "ready", "runtimes": live }))
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
        .get_or_start(&id, &state.command, cols, rows)
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

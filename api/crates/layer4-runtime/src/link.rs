use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{
    ws::{Message, WebSocket},
    DefaultBodyLimit, Path, State, WebSocketUpgrade,
};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt as _, StreamExt as _};
use layer3_adapter::wire::RequestFrame;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::bridge::{Bridge, Outbound, MAX_FRAME_BYTES};
use crate::failure::RuntimeError;
use crate::lanes::{self, Ready};
use crate::supervisor::{Phase, Supervisor};

const CONTROL_BODY_BYTES: usize = 384 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

pub(crate) fn router(supervisor: Arc<Supervisor>) -> Router {
    Router::new()
        .route("/livez", get(liveness))
        .route("/ready", get(readiness))
        .route("/readyz", get(readiness))
        .route(
            "/runtime/control",
            post(control).layer(DefaultBodyLimit::max(CONTROL_BODY_BYTES)),
        )
        .route("/runtime/agents/{agent_id}/conversation", get(conversation))
        .route(
            "/runtime/checkpoints/{reference}",
            get(download_checkpoint).put(upload_checkpoint),
        )
        .with_state(supervisor)
}

async fn liveness() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct Readiness {
    status: &'static str,
    phase: &'static str,
}

async fn readiness(State(supervisor): State<Arc<Supervisor>>) -> Json<Readiness> {
    let phase = match supervisor.phase().await {
        Phase::Provisioned => "provisioned",
        Phase::Installed => "installed",
        Phase::Credentialed => "credentialed",
    };
    Json(Readiness {
        status: "ready",
        phase,
    })
}

async fn control(State(supervisor): State<Arc<Supervisor>>, body: Bytes) -> Response {
    let value = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            return protocol_failure(
                StatusCode::BAD_REQUEST,
                "malformed_frame",
                "request body is not a JSON runtime frame",
                false,
            )
        }
    };
    let Some(tag) = value
        .as_object()
        .and_then(|object| object.get("t"))
        .and_then(serde_json::Value::as_str)
    else {
        return protocol_failure(
            StatusCode::BAD_REQUEST,
            "unknown_frame",
            "runtime frame does not contain a recognized string tag",
            false,
        );
    };
    let Some(operation) = crate::wire_operation::for_tag(tag) else {
        return protocol_failure(
            StatusCode::BAD_REQUEST,
            "unknown_frame",
            "runtime frame tag is not recognized",
            false,
        );
    };
    let request = match serde_json::from_value::<RequestFrame>(value) {
        Ok(request) => request,
        Err(_) => {
            return Json(
                RuntimeError::rejected(
                    "malformed_frame",
                    "runtime frame does not match the shared schema",
                )
                .response(operation),
            )
            .into_response()
        }
    };
    Json(supervisor.dispatch(request).await).into_response()
}

async fn conversation(
    ws: WebSocketUpgrade,
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    let Some(bridge) = supervisor.bridge(&agent_id).await else {
        return protocol_failure(
            StatusCode::NOT_FOUND,
            "agent_not_found",
            "agent process was not found",
            false,
        );
    };
    ws.max_message_size(MAX_FRAME_BYTES + 1)
        .max_frame_size(MAX_FRAME_BYTES + 1)
        .on_upgrade(move |socket| relay_conversation(socket, bridge, agent_id))
        .into_response()
}

async fn relay_conversation(mut socket: WebSocket, bridge: Arc<Bridge>, agent_id: String) {
    let mut live = bridge.subscribe();
    let backlog = bridge.backlog().await;
    let ready = match serde_json::to_string(&Ready::agent(&agent_id)) {
        Ok(ready) => ready,
        Err(error) => {
            tracing::error!(%error, %agent_id, "conversation ready frame could not be encoded");
            return;
        }
    };
    if socket.send(Message::Text(ready.into())).await.is_err() {
        return;
    }
    for held in backlog {
        if socket.send(outbound(held)).await.is_err() {
            return;
        }
    }

    let (sink, mut source) = socket.split();
    let output_bridge = Arc::clone(&bridge);
    let mut output = tokio::spawn(async move {
        pump_output(sink, &mut live, output_bridge, &agent_id).await;
    });
    loop {
        tokio::select! {
            result = &mut output => {
                if let Err(error) = result {
                    tracing::warn!(%error, "conversation output task failed");
                }
                return;
            }
            incoming = source.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match lanes::unframe(&bytes) {
                    Some((lanes::CONVERSATION, payload)) => {
                        if !bridge.send(payload.to_vec()).await {
                            tracing::warn!("conversation bridge stopped accepting input");
                            output.abort();
                            return;
                        }
                    }
                    Some((lane, _)) => {
                        tracing::warn!(lane, "frame arrived on an unsupported conversation lane");
                    }
                    None => {}
                },
                Some(Ok(Message::Close(_))) | None => {
                    output.abort();
                    return;
                }
                Some(Ok(Message::Text(_))) => {
                    tracing::warn!("text frame ignored after the conversation ready frame");
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    tracing::warn!(%error, "conversation WebSocket input failed");
                    output.abort();
                    return;
                }
            }
        }
    }
}

async fn pump_output(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    live: &mut tokio::sync::broadcast::Receiver<Outbound>,
    bridge: Arc<Bridge>,
    agent_id: &str,
) {
    loop {
        let said = tokio::select! {
            _ = bridge.ended() => break,
            said = live.recv() => said,
        };
        let message = match said {
            Ok(said) => outbound(said),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::error!(%agent_id, skipped, "conversation consumer lagged");
                gap(usize::try_from(skipped).unwrap_or(usize::MAX))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if sink.send(message).await.is_err() {
            return;
        }
    }
    let _ = sink.send(Message::Close(None)).await;
}

fn outbound(said: Outbound) -> Message {
    match said {
        Outbound::Frame(bytes) => Message::Binary(lanes::frame(lanes::CONVERSATION, &bytes).into()),
        Outbound::Gap { frames } => gap(frames),
    }
}

fn gap(frames: usize) -> Message {
    let payload = serde_json::json!({ "frames": frames }).to_string();
    Message::Binary(lanes::frame(lanes::CONVERSATION_GAP, payload.as_bytes()).into())
}

async fn download_checkpoint(
    Path(reference): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    let path = match supervisor.checkpoint_store().download_path(&reference) {
        Ok(path) => path,
        Err(error) => return runtime_failure(error),
    };
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) => {
            return runtime_failure(RuntimeError::unavailable(
                "checkpoint_io_failed",
                format!("open checkpoint download: {error}"),
            ))
        }
    };
    let length = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return runtime_failure(RuntimeError::unavailable(
                "checkpoint_io_failed",
                format!("inspect checkpoint download: {error}"),
            ))
        }
    };
    let stream = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            Ok::<Option<(Bytes, tokio::fs::File)>, std::io::Error>(None)
        } else {
            buffer.truncate(read);
            Ok(Some((Bytes::from(buffer), file)))
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

async fn upload_checkpoint(
    Path(reference): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if content_length(&headers)
        .is_some_and(|length| length > crate::checkpoint_archive::MAX_ARCHIVE_BYTES)
    {
        return protocol_failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "checkpoint_too_large",
            "checkpoint upload exceeds the archive size limit",
            false,
        );
    }
    let (temporary, expected) = match supervisor.checkpoint_store().upload_path(&reference) {
        Ok(upload) => upload,
        Err(error) => return runtime_failure(error),
    };
    let mut cleanup = TemporaryUpload::new(temporary.clone());
    let mut file = match tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            return runtime_failure(RuntimeError::unavailable(
                "checkpoint_io_failed",
                format!("create checkpoint upload: {error}"),
            ))
        }
    };
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return runtime_failure(RuntimeError::unavailable(
                    "checkpoint_upload_failed",
                    format!("read checkpoint upload: {error}"),
                ))
            }
        };
        received = match received.checked_add(chunk.len() as u64) {
            Some(received) if received <= crate::checkpoint_archive::MAX_ARCHIVE_BYTES => received,
            _ => {
                return protocol_failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "checkpoint_too_large",
                    "checkpoint upload exceeds the archive size limit",
                    false,
                )
            }
        };
        if let Err(error) = file.write_all(&chunk).await {
            return runtime_failure(RuntimeError::unavailable(
                "checkpoint_io_failed",
                format!("write checkpoint upload: {error}"),
            ));
        }
        digest.update(&chunk);
    }
    if let Err(error) = file.flush().await {
        return runtime_failure(RuntimeError::unavailable(
            "checkpoint_io_failed",
            format!("flush checkpoint upload: {error}"),
        ));
    }
    if let Err(error) = file.sync_all().await {
        return runtime_failure(RuntimeError::unavailable(
            "checkpoint_io_failed",
            format!("sync checkpoint upload: {error}"),
        ));
    }
    drop(file);
    let actual = crate::command::hex(&digest.finalize());
    if !actual.eq_ignore_ascii_case(&expected) {
        return protocol_failure(
            StatusCode::BAD_REQUEST,
            "checkpoint_checksum_mismatch",
            "checkpoint upload did not match its object reference",
            false,
        );
    }
    if let Err(error) = supervisor
        .checkpoint_store()
        .commit_upload(temporary, reference, expected)
        .await
    {
        return runtime_failure(error);
    }
    cleanup.disarm();
    StatusCode::NO_CONTENT.into_response()
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

#[derive(Serialize)]
struct ProtocolFailure {
    t: &'static str,
    code: String,
    detail: String,
    retryable: bool,
}

fn protocol_failure(
    status: StatusCode,
    code: impl Into<String>,
    detail: impl Into<String>,
    retryable: bool,
) -> Response {
    (
        status,
        Json(ProtocolFailure {
            t: "failure",
            code: code.into(),
            detail: detail.into(),
            retryable,
        }),
    )
        .into_response()
}

fn runtime_failure(error: RuntimeError) -> Response {
    let status = if error.code == "checkpoint_not_found" {
        StatusCode::NOT_FOUND
    } else if error.retryable {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_REQUEST
    };
    protocol_failure(status, error.code, error.detail, error.retryable)
}

struct TemporaryUpload(Option<PathBuf>);

impl TemporaryUpload {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;

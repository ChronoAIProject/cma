mod conversation;
mod desktop;
mod desktop_control;
mod ownership;
mod runtime_status;
mod stale_turn;
mod upload;
use conversation::conversation;
use upload::TemporaryUpload;

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt as _;
use layer3_adapter::wire::{
    CheckpointUploadResponse, RequestFrame, CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::failure::RuntimeError;
use crate::supervisor::{Phase, Supervisor};
use layer3_adapter::agent_capacity_recovery::CapacityRecoveryRequest;

const CONTROL_BODY_BYTES: usize = 384 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const CONTROL_HEARTBEAT: Duration = Duration::from_secs(15);

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
            "/runtime/agents/{agent_id}/stale-turn/stop",
            post(stale_turn::stop).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/runtime/agents/{agent_id}/stale-turn/observe",
            post(stale_turn::observe).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/runtime/agents/{agent_id}/manager-ownership/challenge",
            post(ownership::challenge),
        )
        .route(
            "/runtime/agents/{agent_id}/manager-ownership/confirm",
            post(ownership::confirm),
        )
        .route(
            "/runtime/agents/{agent_id}/manager-ownership/revoke",
            post(ownership::revoke),
        )
        .route(
            "/runtime/agents/{agent_id}/manager-ownership/legacy",
            post(ownership::reset_legacy),
        )
        .route(
            "/runtime/agents/{agent_id}/capacity-recovery",
            post(capacity_recovery),
        )
        .route("/runtime/desktop", get(desktop::websocket))
        .route(
            "/runtime/desktop/ownership/challenge",
            post(desktop_control::challenge).layer(DefaultBodyLimit::max(8192)),
        )
        .route(
            "/runtime/desktop/ownership/confirm",
            post(desktop_control::confirm).layer(DefaultBodyLimit::max(8192)),
        )
        .route(
            "/runtime/desktop/ownership/revoke",
            post(desktop_control::revoke).layer(DefaultBodyLimit::max(8192)),
        )
        .route("/runtime/agents/{agent_id}/history", get(history))
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
        Phase::RestorePending => "restore_pending",
        Phase::RestoreCredentialed => "restore_credentialed",
        Phase::Credentialed => "credentialed",
    };
    Json(Readiness {
        status: "ready",
        phase,
    })
}

async fn history(
    Path(agent_id): Path<String>,
    State(supervisor): State<Arc<Supervisor>>,
) -> Response {
    match supervisor.export_history(&agent_id).await {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => runtime_failure(error),
    }
}

#[path = "link/capacity_recovery.rs"]
mod recovery;
use recovery::capacity_recovery;

async fn control(
    State(supervisor): State<Arc<Supervisor>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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
    let owner = match conversation::ownership_header(&headers) {
        Ok(owner) => owner,
        Err(error) => return runtime_failure(error),
    };
    if matches!(
        request,
        RequestFrame::Checkpoint(_)
            | RequestFrame::CheckpointForTeardown(_)
            | RequestFrame::BootstrapRestore(_)
            | RequestFrame::Restore(_)
            | RequestFrame::ReconcileWorkspaceRemoval(_)
    ) {
        let result =
            tokio::spawn(async move { ownership::dispatch(&supervisor, request, owner).await });
        return heartbeat_json(result, CONTROL_HEARTBEAT);
    }
    match ownership::dispatch(&supervisor, request, owner).await {
        layer3_adapter::wire::ResponseFrame::RuntimeStatus(status) => {
            runtime_status::response(&supervisor, &headers, status).await
        }
        response => Json(response).into_response(),
    }
}

fn heartbeat_json<T: Serialize + Send + 'static>(
    result: tokio::task::JoinHandle<T>,
    interval: Duration,
) -> Response {
    let mut heartbeat = tokio::time::interval(interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // JSON whitespace keeps proxy read deadlines alive without changing the frame.
    // Dropping the HTTP body detaches its task, retaining operation and writer guards.
    let stream = futures_util::stream::try_unfold(Some((result, heartbeat)), |state| async move {
        let Some((mut result, mut heartbeat)) = state else {
            return Ok(None);
        };
        tokio::select! {
            biased;
            value = &mut result => {
                let value = value.map_err(std::io::Error::other)?;
                let bytes = serde_json::to_vec(&value).map_err(std::io::Error::other)?;
                Ok::<_, std::io::Error>(Some((Bytes::from(bytes), None)))
            }
            _ = heartbeat.tick() => Ok(Some((Bytes::from_static(b"\n"), Some((result, heartbeat))))),
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
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
    let result = tokio::spawn(async move {
        match supervisor
            .checkpoint_store()
            .commit_upload(temporary, reference.clone(), expected.clone())
            .await
        {
            Ok(()) => {
                cleanup.disarm();
                CheckpointUploadResponse::Committed {
                    object_reference: reference,
                    sha256: expected,
                    size_bytes: received,
                }
            }
            Err(error) => CheckpointUploadResponse::Failure {
                code: error.code,
                detail: error.detail,
                retryable: error.retryable,
            },
        }
    });
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim() == CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE)
        })
    {
        let mut response = heartbeat_json(result, CONTROL_HEARTBEAT);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE),
        );
        return response;
    }
    match result.await {
        Ok(CheckpointUploadResponse::Committed { .. }) => StatusCode::NO_CONTENT.into_response(),
        Ok(CheckpointUploadResponse::Failure {
            code,
            detail,
            retryable,
        }) => runtime_failure(RuntimeError::recorded(code, detail, retryable)),
        Err(error) => runtime_failure(RuntimeError::unavailable(
            "checkpoint_upload_failed",
            format!("checkpoint upload task did not complete: {error}"),
        )),
    }
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

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;

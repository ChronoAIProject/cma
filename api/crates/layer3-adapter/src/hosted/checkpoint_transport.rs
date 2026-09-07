use std::sync::{Arc, Mutex as StdMutex};

use futures_util::{stream, StreamExt as _};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use url::Url;

use super::checkpoint_store::{
    CheckpointArchiveDownload, CheckpointArchiveStore, CheckpointArchiveUpload,
    MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use crate::wire::{CheckpointUploadResponse, CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE};
use crate::{AdapterError, CheckpointReceipt, RestoreRequest};

const ARCHIVE_CHANNEL_CAPACITY: usize = 4;
const ERROR_BODY_BYTES: usize = 64 * 1024;

pub(super) async fn export(
    http: &reqwest::Client,
    url: Url,
    auth: (&str, &str),
    checkpoint: &CheckpointReceipt,
    store: &Arc<dyn CheckpointArchiveStore>,
) -> crate::Result<()> {
    let response = http
        .get(url)
        .header(auth.0, auth.1)
        .send()
        .await
        .map_err(|error| {
            AdapterError::Unavailable(format!("checkpoint download failed: {error}"))
        })?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(response_error("checkpoint download", response).await);
    }
    let size_bytes = exact_content_length(&response)?;
    let (chunks, incoming) = mpsc::channel(ARCHIVE_CHANNEL_CAPACITY);
    let upload = CheckpointArchiveUpload {
        object_reference: checkpoint.object_reference.clone(),
        sha256: checkpoint.sha256.clone(),
        size_bytes,
        chunks: incoming,
    };
    let transfer = pump_runtime_archive(response, chunks, size_bytes, &checkpoint.sha256);
    let commit = store.upload(upload);
    let (transferred, committed) = tokio::join!(transfer, commit);
    let committed = committed?;
    transferred?;
    if committed.object_reference != checkpoint.object_reference
        || committed.sha256 != checkpoint.sha256
        || committed.size_bytes != size_bytes
    {
        return Err(AdapterError::Rejected(
            "checkpoint store committed a different archive identity".into(),
        ));
    }
    Ok(())
}

pub(super) async fn import(
    http: &reqwest::Client,
    url: Url,
    auth: (&str, &str),
    request: &RestoreRequest,
    store: &Arc<dyn CheckpointArchiveStore>,
) -> crate::Result<()> {
    let download = store.download(&request.object_reference).await?;
    validate_download(request, &download)?;
    let size_bytes = download.size_bytes;
    let audit = Arc::new(StdMutex::new(UploadAudit::default()));
    let stream_audit = Arc::clone(&audit);
    let body = reqwest::Body::wrap_stream(stream::unfold(
        (download.chunks, stream_audit, size_bytes),
        |(mut chunks, audit, limit)| async move {
            match chunks.recv().await {
                Some(Ok(chunk)) => {
                    let result = audit
                        .lock()
                        .expect("checkpoint upload audit lock")
                        .record(&chunk, limit)
                        .map(|()| chunk);
                    Some((result.map_err(archive_io), (chunks, audit, limit)))
                }
                Some(Err(error)) => {
                    audit
                        .lock()
                        .expect("checkpoint upload audit lock")
                        .fail(error.clone());
                    Some((Err(archive_io(error)), (chunks, audit, limit)))
                }
                None => {
                    audit.lock().expect("checkpoint upload audit lock").finish();
                    None
                }
            }
        },
    ));
    let response = http
        .put(url)
        .header(auth.0, auth.1)
        .header(
            reqwest::header::ACCEPT,
            CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE,
        )
        .header(reqwest::header::CONTENT_LENGTH, size_bytes)
        .body(body)
        .send()
        .await;
    let outcome = audit
        .lock()
        .expect("checkpoint upload audit lock")
        .outcome(size_bytes, &request.sha256);
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return Err(outcome.err().unwrap_or_else(|| {
                AdapterError::Unavailable(format!("checkpoint upload failed: {error}"))
            }))
        }
    };
    outcome?;
    match response.status() {
        reqwest::StatusCode::NO_CONTENT => Ok(()),
        reqwest::StatusCode::OK => tokio::time::timeout(
            std::time::Duration::from_secs(60 * 60),
            decode_upload_receipt(response, request, size_bytes),
        )
        .await
        .map_err(|_| {
            AdapterError::Unavailable("checkpoint upload acknowledgement timed out".into())
        })?,
        _ => Err(response_error("checkpoint upload", response).await),
    }
}

async fn decode_upload_receipt(
    response: reqwest::Response,
    request: &RestoreRequest,
    size_bytes: u64,
) -> crate::Result<()> {
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE)
    {
        return Err(AdapterError::Rejected(
            "checkpoint upload receipt media type is invalid".into(),
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > ERROR_BODY_BYTES as u64)
    {
        return Err(AdapterError::Rejected(
            "checkpoint upload receipt exceeds its size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            AdapterError::Unavailable("checkpoint upload receipt stream failed".into())
        })?;
        if bytes.len().saturating_add(chunk.len()) > ERROR_BODY_BYTES {
            return Err(AdapterError::Rejected(
                "checkpoint upload receipt exceeds its size limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let receipt: CheckpointUploadResponse = serde_json::from_slice(&bytes)
        .map_err(|_| AdapterError::Rejected("checkpoint upload receipt is malformed".into()))?;
    match receipt {
        CheckpointUploadResponse::Committed {
            object_reference,
            sha256,
            size_bytes: committed_size,
        } if object_reference == request.object_reference
            && sha256 == request.sha256
            && committed_size == size_bytes =>
        {
            Ok(())
        }
        CheckpointUploadResponse::Committed { .. } => Err(AdapterError::Rejected(
            "checkpoint upload receipt does not match the requested archive".into(),
        )),
        CheckpointUploadResponse::Failure {
            code,
            detail,
            retryable,
        } => {
            let message = format!("checkpoint upload failed: {code}: {detail}");
            Err(if retryable {
                AdapterError::Unavailable(message)
            } else {
                AdapterError::Rejected(message)
            })
        }
    }
}

fn validate_download(
    request: &RestoreRequest,
    download: &CheckpointArchiveDownload,
) -> crate::Result<()> {
    if download.object_reference != request.object_reference
        || download.sha256 != request.sha256
        || download.size_bytes == 0
        || download.size_bytes > MAX_CHECKPOINT_ARCHIVE_BYTES
    {
        return Err(AdapterError::Rejected(
            "checkpoint store returned a different or invalid archive identity".into(),
        ));
    }
    Ok(())
}

async fn pump_runtime_archive(
    response: reqwest::Response,
    chunks: mpsc::Sender<crate::Result<Vec<u8>>>,
    expected_size: u64,
    expected_sha256: &str,
) -> crate::Result<()> {
    let mut stream = response.bytes_stream();
    let mut size = 0_u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let error = AdapterError::Unavailable(format!(
                    "checkpoint download stream failed: {error}"
                ));
                let _ = chunks.send(Err(error.clone())).await;
                return Err(error);
            }
        };
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| AdapterError::Rejected("checkpoint download size overflowed".into()))?;
        if size > expected_size || size > MAX_CHECKPOINT_ARCHIVE_BYTES {
            let error =
                AdapterError::Rejected("checkpoint download exceeded its declared size".into());
            let _ = chunks.send(Err(error.clone())).await;
            return Err(error);
        }
        digest.update(&chunk);
        chunks.send(Ok(chunk.to_vec())).await.map_err(|_| {
            AdapterError::Unavailable("checkpoint store stopped accepting the archive".into())
        })?;
    }
    let actual_sha256 = hex(&digest.finalize());
    if size != expected_size || actual_sha256 != expected_sha256 {
        let error =
            AdapterError::Rejected("checkpoint download did not match its sealed identity".into());
        let _ = chunks.send(Err(error.clone())).await;
        return Err(error);
    }
    Ok(())
}

fn exact_content_length(response: &reqwest::Response) -> crate::Result<u64> {
    let value = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .ok_or_else(|| {
            AdapterError::Rejected("checkpoint download has no content length".into())
        })?;
    let size = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|size| *size > 0 && *size <= MAX_CHECKPOINT_ARCHIVE_BYTES)
        .ok_or_else(|| {
            AdapterError::Rejected("checkpoint download content length is invalid".into())
        })?;
    Ok(size)
}

#[derive(Default)]
struct UploadAudit {
    digest: Sha256,
    size: u64,
    finished: bool,
    failure: Option<AdapterError>,
}

impl UploadAudit {
    fn record(&mut self, chunk: &[u8], expected: u64) -> crate::Result<()> {
        let size = self
            .size
            .checked_add(chunk.len() as u64)
            .filter(|size| *size <= expected && *size <= MAX_CHECKPOINT_ARCHIVE_BYTES)
            .ok_or_else(|| AdapterError::Rejected("checkpoint store stream is too large".into()));
        let size = match size {
            Ok(size) => size,
            Err(error) => {
                self.failure = Some(error.clone());
                return Err(error);
            }
        };
        self.size = size;
        self.digest.update(chunk);
        // Hyper need not poll the body stream for EOF after consuming the
        // declared Content-Length, so the exact byte count is completion.
        self.finished = self.size == expected;
        Ok(())
    }

    fn fail(&mut self, error: AdapterError) {
        self.failure = Some(error);
    }

    fn finish(&mut self) {
        self.finished = true;
    }

    fn outcome(&self, expected_size: u64, expected_sha256: &str) -> crate::Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let digest = hex(&self.digest.clone().finalize());
        if !self.finished || self.size != expected_size || digest != expected_sha256 {
            return Err(AdapterError::Rejected(
                "checkpoint store stream did not match its declared identity".into(),
            ));
        }
        Ok(())
    }
}

fn archive_io(error: AdapterError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

async fn response_error(operation: &str, response: reqwest::Response) -> AdapterError {
    let status = response.status();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        if body.len().saturating_add(chunk.len()) > ERROR_BODY_BYTES {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let detail = String::from_utf8_lossy(&body);
    let message = format!("{operation} returned {status}: {detail}");
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        AdapterError::Unavailable(message)
    } else {
        AdapterError::Rejected(message)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
#[path = "checkpoint_transport_tests.rs"]
mod tests;

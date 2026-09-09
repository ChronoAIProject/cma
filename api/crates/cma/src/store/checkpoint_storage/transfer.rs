use std::sync::{Arc, Mutex};

use futures_util::{stream, StreamExt};
use layer3_adapter::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::{rejected, unavailable, ChronoStorage, Result};

pub(super) const CHUNK_BYTES: usize = 256 * 1024;

pub(super) async fn upload(
    store: &ChronoStorage,
    key: &str,
    archive: CheckpointArchiveUpload,
) -> Result<()> {
    let CheckpointArchiveUpload {
        object_reference,
        sha256,
        size_bytes,
        chunks,
    } = archive;
    if !(1..=MAX_CHECKPOINT_ARCHIVE_BYTES).contains(&size_bytes) {
        return Err(rejected("checkpoint archive size is invalid"));
    }
    let seal = Arc::new(Mutex::new(None));
    let outcome = seal.clone();
    let expected_sha = sha256.clone();
    let body = stream::unfold(
        (chunks, Sha256::new(), 0_u64, false),
        move |(mut chunks, mut digest, size, stopped)| {
            let outcome = outcome.clone();
            let expected_sha = expected_sha.clone();
            async move {
                if stopped {
                    return None;
                }
                match chunks.recv().await {
                    Some(Ok(chunk)) if size.saturating_add(chunk.len() as u64) <= size_bytes => {
                        digest.update(&chunk);
                        let next = size + chunk.len() as u64;
                        if next == size_bytes {
                            // Known-length HTTP bodies need not be polled again after their last bytes.
                            let valid = chunks.recv().await.is_none()
                                && format!("{:x}", digest.clone().finalize()) == expected_sha;
                            if !valid {
                                *outcome.lock().expect("upload seal lock") = Some(Err(rejected(
                                    "checkpoint archive upload failed integrity verification",
                                )));
                                return Some((
                                    Err(std::io::Error::other("checkpoint archive stream failed")),
                                    (chunks, digest, next, true),
                                ));
                            }
                            *outcome.lock().expect("upload seal lock") = Some(Ok(()));
                        }
                        Some((
                            Ok::<_, std::io::Error>(chunk),
                            (chunks, digest, next, false),
                        ))
                    }
                    Some(_) => {
                        *outcome.lock().expect("upload seal lock") =
                            Some(Err(rejected("checkpoint archive upload stream is invalid")));
                        Some((
                            Err(std::io::Error::other("checkpoint archive stream failed")),
                            (chunks, digest, size, true),
                        ))
                    }
                    None => {
                        let valid = size == size_bytes
                            && format!("{:x}", digest.finalize()) == expected_sha;
                        *outcome.lock().expect("upload seal lock") = Some(if valid {
                            Ok(())
                        } else {
                            Err(rejected(
                                "checkpoint archive upload failed integrity verification",
                            ))
                        });
                        None
                    }
                }
            }
        },
    );
    let response = store
        .http
        .post(store.objects.clone())
        .query(&[
            ("key", key),
            ("contentType", "application/vnd.chrono.cma-checkpoint.v1"),
        ])
        .bearer_auth(store.tokens.token().await?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, size_bytes)
        .body(reqwest::Body::wrap_stream(body))
        .send()
        .await
        .map_err(|_| unavailable("checkpoint storage upload is uncertain"))?;
    if !response.status().is_success() {
        return Err(layer3_adapter::AdapterError::Unavailable(format!(
            "checkpoint storage upload was rejected (HTTP {})",
            response.status().as_u16()
        )));
    }
    seal.lock()
        .expect("upload seal lock")
        .take()
        .ok_or_else(|| {
            unavailable("checkpoint storage upload did not consume the complete stream")
        })??;
    let mut retained = download(store, key, &object_reference, &sha256, size_bytes)
        .await?
        .chunks;
    while let Some(chunk) = retained.recv().await {
        chunk?;
    }
    Ok(())
}

pub(super) async fn download(
    store: &ChronoStorage,
    key: &str,
    reference: &str,
    sha256: &str,
    size: u64,
) -> Result<CheckpointArchiveDownload> {
    let response = download_response(store, key, size).await?;
    let (sender, chunks) = mpsc::channel(2);
    let expected = sha256.to_owned();
    tokio::spawn(async move {
        tokio::select! {
            _ = sender.closed() => {}
            result = stream_download(response, size, &expected, &sender) => {
                if let Err(error) = result {
                    let _ = sender.send(Err(error)).await;
                }
            }
        }
    });
    Ok(CheckpointArchiveDownload {
        object_reference: reference.to_owned(),
        sha256: sha256.to_owned(),
        size_bytes: size,
        chunks,
    })
}

pub(super) async fn download_response(
    store: &ChronoStorage,
    key: &str,
    size: u64,
) -> Result<reqwest::Response> {
    if !(1..=MAX_CHECKPOINT_ARCHIVE_BYTES).contains(&size) {
        return Err(rejected("checkpoint archive size is invalid"));
    }
    let response = store
        .http
        .get(store.download.clone())
        .query(&[("key", key)])
        .bearer_auth(store.tokens.token().await?)
        .send()
        .await
        .map_err(|_| unavailable("checkpoint storage download failed"))?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length != size)
    {
        return Err(unavailable(
            "checkpoint storage download status or size is invalid",
        ));
    }
    Ok(response)
}

async fn stream_download(
    response: reqwest::Response,
    expected_size: u64,
    expected_sha: &str,
    sender: &mpsc::Sender<Result<Vec<u8>>>,
) -> Result<()> {
    let mut stream = response.bytes_stream();
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| unavailable("checkpoint storage download stream failed"))?;
        size = size
            .checked_add(chunk.len() as u64)
            .filter(|size| *size <= expected_size)
            .ok_or_else(|| unavailable("checkpoint storage download exceeds its declared size"))?;
        digest.update(&chunk);
        for part in chunk.chunks(CHUNK_BYTES) {
            if sender.send(Ok(part.to_vec())).await.is_err() {
                return Ok(());
            }
        }
    }
    if size != expected_size || format!("{:x}", digest.finalize()) != expected_sha {
        return Err(unavailable(
            "checkpoint storage download failed integrity verification",
        ));
    }
    Ok(())
}

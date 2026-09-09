use futures_util::{stream, stream::FuturesOrdered, StreamExt};
use layer3_adapter::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::{part_transfer, rejected, transfer, unavailable, ChronoStorage, Result};

pub(super) const PART_BYTES: usize = 32 * 1024 * 1024;
pub(super) const UPLOAD_CONCURRENCY: usize = 4;
pub(super) const DOWNLOAD_CONCURRENCY: usize = 4;
const MANIFEST_BYTES: usize = 64 * 1024;
const SCHEMA: &str = "cma-checkpoint-parts-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    size_bytes: u64,
    sha256: String,
    parts: Vec<String>,
}

pub(super) fn size_from_key(key: &str) -> Result<Option<u64>> {
    let Some((_, value)) = key.rsplit_once(".parts-v1-") else {
        return Ok(None);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|size| *size > PART_BYTES as u64 && *size <= MAX_CHECKPOINT_ARCHIVE_BYTES)
        .filter(|size| size.to_string() == value)
        .map(Some)
        .ok_or_else(|| rejected("checkpoint part layout is invalid"))
}

pub(super) fn part_key(key: &str, index: u64) -> String {
    format!("{key}/{index:04}")
}

pub(super) async fn upload(
    store: &ChronoStorage,
    key: &str,
    expected_size: u64,
    mut archive: CheckpointArchiveUpload,
) -> Result<()> {
    if archive.size_bytes != expected_size {
        return Err(rejected("checkpoint part layout has a different size"));
    }
    let mut manifest = Manifest {
        schema: SCHEMA.into(),
        size_bytes: expected_size,
        sha256: archive.sha256,
        parts: Vec::new(),
    };
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = Vec::with_capacity(PART_BYTES);
    // Keep at most four parts in flight; ordered completion also fixes manifest order.
    let mut pending = FuturesOrdered::new();
    let mut next_part = 0_u64;
    loop {
        let chunk = tokio::select! {
            result = pending.next(), if !pending.is_empty() => {
                manifest.parts.push(result.expect("pending part remains present")?);
                continue;
            }
            chunk = archive.chunks.recv() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        size = size
            .checked_add(chunk.len() as u64)
            .filter(|size| *size <= expected_size)
            .ok_or_else(|| rejected("checkpoint archive exceeds its declared size"))?;
        digest.update(&chunk);
        let mut remaining = chunk.as_slice();
        while !remaining.is_empty() {
            let taken = remaining.len().min(PART_BYTES - buffer.len());
            buffer.extend_from_slice(&remaining[..taken]);
            remaining = &remaining[taken..];
            if buffer.len() == PART_BYTES {
                let bytes = std::mem::replace(&mut buffer, Vec::with_capacity(PART_BYTES));
                pending.push_back(upload_part(store, part_key(key, next_part), bytes));
                next_part += 1;
                if pending.len() == UPLOAD_CONCURRENCY {
                    manifest
                        .parts
                        .push(pending.next().await.expect("upload window is full")?);
                }
            }
        }
    }
    let verified = size == expected_size && format!("{:x}", digest.finalize()) == manifest.sha256;
    if verified && !buffer.is_empty() {
        pending.push_back(upload_part(store, part_key(key, next_part), buffer));
    }
    while let Some(result) = pending.next().await {
        manifest.parts.push(result?);
    }
    if !verified {
        return Err(rejected(
            "checkpoint archive upload failed integrity verification",
        ));
    }
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|_| rejected("checkpoint part manifest is invalid"))?;
    if bytes.len() > MANIFEST_BYTES {
        return Err(rejected("checkpoint part manifest exceeds its bound"));
    }
    upload_bytes(store, key, bytes).await?;
    Ok(())
}

async fn upload_part(store: &ChronoStorage, key: String, bytes: Vec<u8>) -> Result<String> {
    upload_bytes(store, &key, bytes).await
}

async fn upload_bytes(store: &ChronoStorage, key: &str, bytes: Vec<u8>) -> Result<String> {
    part_transfer::upload(store, key, bytes).await
}

pub(super) async fn download(
    store: &ChronoStorage,
    key: &str,
    reference: &str,
    sha256: &str,
    size: u64,
) -> Result<CheckpointArchiveDownload> {
    let response = store
        .http
        .get(store.download.clone())
        .query(&[("key", key)])
        .bearer_auth(store.tokens.token().await?)
        .send()
        .await
        .map_err(|_| unavailable("checkpoint part manifest download failed"))?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MANIFEST_BYTES as u64)
    {
        return Err(unavailable(
            "checkpoint part manifest status or size is invalid",
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| unavailable("checkpoint part manifest download failed"))?;
        if bytes.len().saturating_add(chunk.len()) > MANIFEST_BYTES {
            return Err(rejected("checkpoint part manifest exceeds its bound"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| rejected("checkpoint part manifest is invalid"))?;
    if manifest.schema != SCHEMA
        || manifest.size_bytes != size
        || manifest.sha256 != sha256
        || manifest.parts.len() as u64 != size.div_ceil(PART_BYTES as u64)
        || manifest.parts.iter().any(|sha| {
            sha.len() != 64
                || !sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
    {
        return Err(rejected("checkpoint part manifest identity is invalid"));
    }
    let (sender, chunks) = mpsc::channel(2);
    let store = store.clone();
    let key = key.to_owned();
    tokio::spawn(async move {
        tokio::select! {
            _ = sender.closed() => {}
            result = download_parts(&store, &key, manifest, &sender) => {
                if let Err(error) = result {
                    let _ = sender.send(Err(error)).await;
                }
            }
        }
    });
    Ok(CheckpointArchiveDownload {
        object_reference: reference.into(),
        sha256: sha256.into(),
        size_bytes: size,
        chunks,
    })
}

async fn download_parts(
    store: &ChronoStorage,
    key: &str,
    manifest: Manifest,
    sender: &mpsc::Sender<Result<Vec<u8>>>,
) -> Result<()> {
    let mut digest = Sha256::new();
    // Ordered buffering holds at most four verified 32 MiB parts, including the
    // part being delivered. No next window is polled while the consumer is full.
    let mut parts = stream::iter(manifest.parts.into_iter().enumerate())
        .map(|(index, sha)| {
            let part_size =
                (manifest.size_bytes - index as u64 * PART_BYTES as u64).min(PART_BYTES as u64);
            download_part(store, part_key(key, index as u64), sha, part_size)
        })
        .buffered(DOWNLOAD_CONCURRENCY);
    while let Some(part) = parts.next().await {
        let part = part?;
        digest.update(&part);
        for chunk in part.chunks(transfer::CHUNK_BYTES) {
            if sender.send(Ok(chunk.to_vec())).await.is_err() {
                return Ok(());
            }
        }
    }
    if format!("{:x}", digest.finalize()) != manifest.sha256 {
        return Err(rejected(
            "checkpoint archive download failed integrity verification",
        ));
    }
    Ok(())
}

async fn download_part(
    store: &ChronoStorage,
    key: String,
    sha: String,
    size: u64,
) -> Result<Vec<u8>> {
    let response = transfer::download_response(store, &key, size).await?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::with_capacity(size as usize);
    let mut digest = Sha256::new();
    // Keep the body in this future so cancellation drops the entire window.
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| unavailable("checkpoint part download stream failed"))?;
        if bytes.len().saturating_add(chunk.len()) as u64 > size {
            return Err(rejected(
                "checkpoint part download exceeds its declared size",
            ));
        }
        digest.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != size || format!("{:x}", digest.finalize()) != sha {
        return Err(rejected(
            "checkpoint part download failed integrity verification",
        ));
    }
    Ok(bytes)
}

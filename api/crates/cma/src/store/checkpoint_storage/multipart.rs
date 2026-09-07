use futures_util::StreamExt;
use layer3_adapter::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveUpload, MAX_CHECKPOINT_ARCHIVE_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::{rejected, transfer, unavailable, ChronoStorage, Result};

pub(super) const PART_BYTES: usize = 32 * 1024 * 1024;
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
    while let Some(chunk) = archive.chunks.recv().await {
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
                let sha =
                    upload_bytes(store, &part_key(key, manifest.parts.len() as u64), bytes).await?;
                manifest.parts.push(sha);
            }
        }
    }
    if size != expected_size || format!("{:x}", digest.finalize()) != manifest.sha256 {
        return Err(rejected(
            "checkpoint archive upload failed integrity verification",
        ));
    }
    if !buffer.is_empty() {
        let sha = upload_bytes(store, &part_key(key, manifest.parts.len() as u64), buffer).await?;
        manifest.parts.push(sha);
    }
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|_| rejected("checkpoint part manifest is invalid"))?;
    if bytes.len() > MANIFEST_BYTES {
        return Err(rejected("checkpoint part manifest exceeds its bound"));
    }
    upload_bytes(store, key, bytes).await?;
    Ok(())
}

async fn upload_bytes(store: &ChronoStorage, key: &str, bytes: Vec<u8>) -> Result<String> {
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let size_bytes = bytes.len() as u64;
    let (sender, chunks) = mpsc::channel(1);
    sender
        .try_send(Ok(bytes))
        .map_err(|_| unavailable("checkpoint part stream is unavailable"))?;
    drop(sender);
    transfer::upload(
        store,
        key,
        CheckpointArchiveUpload {
            object_reference: format!("home-{sha256}.cma"),
            sha256: sha256.clone(),
            size_bytes,
            chunks,
        },
    )
    .await?;
    Ok(sha256)
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
        if let Err(error) = download_parts(&store, &key, manifest, &sender).await {
            let _ = sender.send(Err(error)).await;
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
    for (index, sha) in manifest.parts.iter().enumerate() {
        let part_size =
            (manifest.size_bytes - index as u64 * PART_BYTES as u64).min(PART_BYTES as u64);
        let mut download =
            transfer::download(store, &part_key(key, index as u64), "part", sha, part_size).await?;
        while let Some(chunk) = download.chunks.recv().await {
            let chunk = chunk?;
            digest.update(&chunk);
            if sender.send(Ok(chunk)).await.is_err() {
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

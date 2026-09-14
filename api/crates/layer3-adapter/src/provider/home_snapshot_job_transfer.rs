use std::time::Duration;

use kube::api::{AttachParams, AttachedProcess};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

use crate::hosted::{CheckpointArchiveStore, CheckpointArchiveUpload};
use crate::{
    AdapterError, HomeSnapshotExportJobObservation, HomeSnapshotExportJobRequest,
    HomeSnapshotExportJobStore, HomeSnapshotExportReceipt, Result,
};

use super::{invalid, spec, PersistentHomeVolumes, Phase};

const MAX_RECEIPT_BYTES: u64 = 256 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(1_800);
// Positional arguments keep paths and identities out of shell source. The
// downward-API UID closes the GET/exec race if a same-named Pod was replaced.
const READ_FILE: &str =
    "[ \"$CMA_EXPORT_POD_UID\" = \"$1\" ] || exit 75; exec /usr/bin/head -c \"$3\" -- \"$2\"";

pub(super) async fn upload(
    store: &PersistentHomeVolumes,
    request: &HomeSnapshotExportJobRequest,
    ready: &HomeSnapshotExportJobObservation,
    archive_store: &dyn CheckpointArchiveStore,
) -> Result<HomeSnapshotExportReceipt> {
    tokio::time::timeout(
        TRANSFER_TIMEOUT,
        upload_bounded(store, request, ready, archive_store),
    )
    .await
    .map_err(|_| unavailable())?
}

async fn upload_bounded(
    store: &PersistentHomeVolumes,
    request: &HomeSnapshotExportJobRequest,
    ready: &HomeSnapshotExportJobObservation,
    archive_store: &dyn CheckpointArchiveStore,
) -> Result<HomeSnapshotExportReceipt> {
    verify_ready(store, request, ready).await?;
    let mut process = read_file(
        store,
        ready,
        &format!("{}/export.json", spec::OUTPUT),
        MAX_RECEIPT_BYTES,
    )
    .await?;
    let mut bytes = Vec::new();
    process
        .stdout()
        .ok_or_else(unavailable)?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(invalid());
    }
    successful(process).await?;
    let receipt: HomeSnapshotExportReceipt =
        serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    receipt.validate_for(&request.export)?;
    verify_ready(store, request, ready).await?;
    let mut process = read_file(
        store,
        ready,
        &format!("{}/{}", spec::OUTPUT, receipt.checkpoint.object_reference),
        receipt.archive_bytes,
    )
    .await?;
    let reader = process.stdout().ok_or_else(unavailable)?;
    let (send, chunks) = mpsc::channel(4);
    let archive = CheckpointArchiveUpload {
        object_reference: receipt.checkpoint.object_reference.clone(),
        sha256: receipt.checkpoint.sha256.clone(),
        size_bytes: receipt.archive_bytes,
        chunks,
    };
    let transfer = async {
        let result = async {
            pump(
                reader,
                &send,
                receipt.archive_bytes,
                &receipt.checkpoint.sha256,
            )
            .await?;
            successful(process).await?;
            verify_ready(store, request, ready).await
        }
        .await;
        if let Err(error) = &result {
            let _ = send.send(Err(error.clone())).await;
        }
        // EOF is published only after full bytes, remote success and exact Pod
        // identity are proved. A failed read cannot look like a complete upload.
        drop(send);
        result
    };
    let (transferred, uploaded) = tokio::join!(transfer, archive_store.upload(archive));
    transferred?;
    let uploaded = uploaded?;
    if uploaded.object_reference != receipt.checkpoint.object_reference
        || uploaded.sha256 != receipt.checkpoint.sha256
        || uploaded.size_bytes != receipt.archive_bytes
    {
        return Err(invalid());
    }
    Ok(receipt)
}

async fn verify_ready(
    store: &PersistentHomeVolumes,
    request: &HomeSnapshotExportJobRequest,
    ready: &HomeSnapshotExportJobObservation,
) -> Result<()> {
    if ready.phase != Phase::Ready
        || ready.view.bound_volume.is_none()
        || store.observe_export_job(request, ready).await? != *ready
    {
        return Err(invalid());
    }
    Ok(())
}

async fn read_file(
    store: &PersistentHomeVolumes,
    ready: &HomeSnapshotExportJobObservation,
    path: &str,
    maximum: u64,
) -> Result<AttachedProcess> {
    let limit = maximum.checked_add(1).ok_or_else(invalid)?.to_string();
    let mut attach = AttachParams::default()
        .container("transfer")
        .stdin(false)
        .stdout(true)
        .stderr(false)
        .tty(false);
    attach.max_stdout_buf_size = Some(64 * 1024);
    store
        .export_pods
        .exec(
            &ready.receipt.pod_reference,
            [
                "/bin/sh",
                "-c",
                READ_FILE,
                "cma-export-read",
                &ready.receipt.pod_generation,
                path,
                &limit,
            ],
            &attach,
        )
        .await
        .map_err(|_| unavailable())
}

async fn successful(mut process: AttachedProcess) -> Result<()> {
    let status = process.take_status().ok_or_else(unavailable)?;
    let (status, joined) = tokio::join!(status, process.join());
    joined.map_err(|_| unavailable())?;
    if status.is_none_or(|status| status.status.as_deref() != Some("Success")) {
        return Err(unavailable());
    }
    Ok(())
}

async fn pump(
    mut reader: impl AsyncRead + Unpin,
    send: &mpsc::Sender<Result<Vec<u8>>>,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<()> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).await.map_err(|_| unavailable())?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .filter(|size| *size <= expected_size)
            .ok_or_else(invalid)?;
        digest.update(&buffer[..count]);
        send.send(Ok(buffer[..count].to_vec()))
            .await
            .map_err(|_| unavailable())?;
    }
    if size != expected_size || format!("{:x}", digest.finalize()) != expected_sha256 {
        return Err(invalid());
    }
    Ok(())
}

fn unavailable() -> AdapterError {
    AdapterError::Unavailable("snapshot export file transfer did not complete".into())
}

#[cfg(test)]
#[path = "home_snapshot_job_transfer_tests.rs"]
mod tests;

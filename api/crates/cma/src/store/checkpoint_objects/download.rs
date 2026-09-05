use futures_util::io::AsyncReadExt;
use layer3_adapter::hosted::CheckpointArchiveDownload;
use layer3_adapter::Result as AdapterResult;
use mongodb::bson::Bson;
use mongodb::gridfs::GridFsDownloadStream;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::model::{
    find_manifest, find_validated_file, finish_sha256, validate_reference, ManifestDoc,
    STREAM_CHANNEL_CAPACITY, STREAM_CHUNK_BYTES,
};
use super::{rejected, unavailable, MongoCheckpointObjectStore};

pub(super) async fn download(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    object_reference: &str,
) -> AdapterResult<CheckpointArchiveDownload> {
    validate_reference(object_reference)?;
    let manifest = find_manifest(store, sandbox_id, object_reference)
        .await?
        .ok_or_else(|| rejected("checkpoint archive was not found"))?;
    if manifest.sandbox_id != sandbox_id || manifest.object_reference != object_reference {
        return Err(unavailable(
            "checkpoint archive manifest ownership is corrupt",
        ));
    }
    find_validated_file(store, &manifest).await?;
    let stream = store
        .bucket
        .open_download_stream(Bson::ObjectId(manifest.gridfs_id))
        .await
        .map_err(|_| unavailable("checkpoint archive download could not start"))?;

    let (sender, chunks) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let response = CheckpointArchiveDownload {
        object_reference: manifest.object_reference.clone(),
        sha256: manifest.sha256.clone(),
        size_bytes: manifest.size_bytes as u64,
        chunks,
    };
    tokio::spawn(stream_file(stream, manifest, sender));
    Ok(response)
}

async fn stream_file(
    mut stream: GridFsDownloadStream,
    manifest: ManifestDoc,
    sender: mpsc::Sender<AdapterResult<Vec<u8>>>,
) {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(read) => read,
            Err(_) => {
                send_error(&sender, "checkpoint archive download read failed").await;
                return;
            }
        };
        if read == 0 {
            if size != manifest.size_bytes as u64 || finish_sha256(digest) != manifest.sha256 {
                send_error(
                    &sender,
                    "checkpoint archive download failed integrity verification",
                )
                .await;
            }
            return;
        }

        let read = read as u64;
        let Some(next_size) = size.checked_add(read) else {
            send_error(
                &sender,
                "checkpoint archive download failed integrity verification",
            )
            .await;
            return;
        };
        if next_size > manifest.size_bytes as u64 {
            send_error(
                &sender,
                "checkpoint archive download failed integrity verification",
            )
            .await;
            return;
        }
        let bytes = buffer[..read as usize].to_vec();
        digest.update(&bytes);
        size = next_size;
        if sender.send(Ok(bytes)).await.is_err() {
            return;
        }
    }
}

async fn send_error(sender: &mpsc::Sender<AdapterResult<Vec<u8>>>, message: &'static str) {
    let _ = sender.send(Err(unavailable(message))).await;
}

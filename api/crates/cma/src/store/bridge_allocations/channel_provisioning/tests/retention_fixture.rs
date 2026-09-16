use super::super::super::super::home;
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::Sandbox;
use layer3_adapter::hosted::{CheckpointArchiveDownload, CheckpointArchiveUpload};
use sha2::{Digest, Sha256};

pub(super) const ARCHIVE: &[u8] = b"retained Channel root checkpoint bytes";

pub(super) async fn with_persistent_home() -> RetirementFixture {
    let original = home::seed().await;
    let f = RetirementFixture::from_renewal(RenewalFixture::from_original(original).await);
    f.start().await;
    for _ in 0..5 {
        f.step().await;
    }
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeGone
    );
    f
}

pub(super) fn upload(bytes: &[u8]) -> CheckpointArchiveUpload {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let (send, chunks) = tokio::sync::mpsc::channel(1);
    send.try_send(Ok(bytes.to_vec())).unwrap();
    CheckpointArchiveUpload {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        size_bytes: bytes.len() as u64,
        chunks,
    }
}

pub(super) async fn bytes(mut value: CheckpointArchiveDownload) -> Vec<u8> {
    let mut result = Vec::new();
    while let Some(chunk) = value.chunks.recv().await {
        result.extend(chunk.unwrap());
    }
    result
}

pub(super) async fn replace_sandbox(f: &RetirementFixture, mut next: Sandbox) {
    let revision = next.revision;
    next.revision += 1;
    next.work.lease = None;
    next.work.ready_at = layer2_core::now_rfc3339();
    let fields = crate::store::sandboxes::sandbox_replacement_fields(&next).unwrap();
    assert_eq!(
        f.db()
            .collection::<Document>("sandboxes")
            .update_one(
                doc! {"_id":next.id.as_str(),"revision":revision as i64},
                doc! {"$set":fields},
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
}

pub(super) async fn with_checkpoint() -> RetirementFixture {
    let f = RetirementFixture::new(0).await;
    let mut sandbox = f.sandbox().await;
    let receipt =
        f.f.original
            .resources
            .checkpoint_objects
            .for_sandbox(sandbox.id.as_str())
            .unwrap()
            .upload(upload(ARCHIVE))
            .await
            .unwrap();
    let checkpoint = layer2_core::sandbox::SandboxCheckpointRef::seal(
        &layer3_adapter::CheckpointReceipt {
            epoch: 1,
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
            writers: layer3_adapter::WriterState::Held,
        },
        &[],
        &sandbox.updated_at,
    )
    .unwrap();
    // Populate a valid retained checkpoint before retirement. Snapshot capture
    // itself is exercised by its separate runtime/persistence contract suites.
    f.db()
        .collection::<Document>("checkpoints")
        .insert_one(doc! {
            "_id":format!("{}:{:020}",sandbox.id,sandbox.revision + 1),
            "document_schema_version":crate::store::schema::DOCUMENT_SCHEMA_VERSION,
            "sandbox_id":sandbox.id.as_str(),
            "sandbox_revision":(sandbox.revision + 1) as i64,
            "sealed_at":&checkpoint.sealed_at,
            "record":crate::store::sandbox_mongo::record_bson(&checkpoint).unwrap(),
        })
        .await
        .unwrap();
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint);
    replace_sandbox(&f, sandbox).await;
    f.start().await;
    for _ in 0..5 {
        f.step().await;
    }
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::RuntimeGone
    );
    f
}

pub(super) async fn retention(f: &RetirementFixture) -> Option<ChannelStorageRetention> {
    let row = f
        .db()
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":f.sandbox().await.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    row.get("channel_storage_retention")
        .cloned()
        .map(|value| mongodb::bson::deserialize_from_bson(value).unwrap())
}

pub(super) async fn rows(f: &RetirementFixture, collection: &str) -> Vec<Document> {
    f.db()
        .collection::<Document>(collection)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

use super::*;
use crate::store::CheckpointBlobStore;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct FinalDeleteStore {
    delegate: Arc<dyn CheckpointBlobStore>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl CheckpointBlobStore for FinalDeleteStore {
    async fn upload(&self, key: &str, archive: CheckpointArchiveUpload) -> AdapterResult<()> {
        self.delegate.upload(key, archive).await
    }
    async fn download(
        &self,
        key: &str,
        reference: &str,
        sha256: &str,
        size: u64,
    ) -> AdapterResult<CheckpointArchiveDownload> {
        self.delegate.download(key, reference, sha256, size).await
    }
    async fn delete(&self, key: &str) -> AdapterResult<()> {
        self.delegate.delete(key).await
    }
    async fn finalize_delete(&self, key: &str) -> AdapterResult<bool> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.delete(key).await?;
        Ok(true)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn confirmed_fence_ends_legacy_tombstone_retries_after_restart() {
    let (client, base, database) = test_store("fence_settle").await;
    let archive = b"uncertain upload";
    let (reference, digest) = sealed(archive);
    let identity = ArchiveIdentity::from_input(reference, digest, archive.len() as u64).unwrap();
    let stage = ManifestDoc::new("sbx_fence_settlement", &identity, DateTime::now());
    super::super::publication::admit(&base, &stage)
        .await
        .unwrap();
    base.objects
        .upload(&stage.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    base.cleanup_due(stage.cleanup_after.to_system_time())
        .await
        .unwrap();
    assert_eq!(base.tombstones.count_documents(doc! {}).await.unwrap(), 1);
    let storage = Arc::new(FinalDeleteStore {
        delegate: base.objects.clone(),
        calls: AtomicUsize::new(0),
    });
    drop(base);
    let store = MongoCheckpointObjectStore::open_with_storage(&client, &database, storage.clone())
        .await
        .unwrap();
    let due = stage.cleanup_after.to_system_time() + std::time::Duration::from_secs(3_600);
    assert_eq!(store.cleanup_due(due).await.unwrap(), 1);
    let settled = store
        .tombstones
        .find_one(doc! {"_id": &stage.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.get_str("object_key").unwrap(), stage.object_key);
    assert!(settled.get_datetime("deletion_fenced_at").is_ok());
    assert!(!settled.contains_key("next_attempt_at"));
    drop(store);
    let reopened =
        MongoCheckpointObjectStore::open_with_storage(&client, &database, storage.clone())
            .await
            .unwrap();
    for day in 1..=7 {
        assert_eq!(
            reopened
                .cleanup_due(due + std::time::Duration::from_secs(day * 86_400))
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(storage.calls.load(Ordering::SeqCst), 1);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn confirmed_fence_cleans_uncertain_stage_without_creating_a_tombstone() {
    let (client, base, database) = test_store("fenced_stage").await;
    let storage = Arc::new(FinalDeleteStore {
        delegate: base.objects.clone(),
        calls: AtomicUsize::new(0),
    });
    let store = MongoCheckpointObjectStore::open_with_storage(&client, &database, storage.clone())
        .await
        .unwrap();
    let archive = b"uncertain stage";
    let (reference, digest) = sealed(archive);
    let identity = ArchiveIdentity::from_input(reference, digest, archive.len() as u64).unwrap();
    let stage = ManifestDoc::new("sbx_fenced_stage", &identity, DateTime::now());
    super::super::publication::admit(&store, &stage)
        .await
        .unwrap();
    store
        .objects
        .upload(&stage.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    assert_eq!(
        store
            .cleanup_due(stage.cleanup_after.to_system_time())
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.manifests.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(store.tombstones.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(storage.calls.load(Ordering::SeqCst), 1);
    client.database(&database).drop().await.unwrap();
}

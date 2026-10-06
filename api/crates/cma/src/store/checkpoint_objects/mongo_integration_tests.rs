use layer2_core::sandbox::store::{
    SandboxCheckpointMutation, SandboxLifecycleExecutionStore, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxWorkState, SandboxWorkStep,
};
use layer3_adapter::hosted::{CheckpointArchiveDownload, CheckpointArchiveUpload};
use layer3_adapter::{AdapterError, CheckpointReceipt, Result as AdapterResult, WriterState};
use mongodb::bson::{doc, DateTime};
use mongodb::options::ClientOptions;
use mongodb::Client;
use tokio::sync::mpsc;

use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::sandbox;

use super::model::{sha256_hex, ArchiveIdentity, ManifestDoc};
use super::MongoCheckpointObjectStore;
#[path = "channel_retention_tests.rs"]
mod channel_retention;
#[path = "deletion_fence_tests.rs"]
mod deletion_fence_tests;
#[path = "legacy_history_tests.rs"]
mod legacy_history_tests;

struct FailedDeleteStore {
    delegate: std::sync::Arc<dyn crate::store::CheckpointBlobStore>,
    fail_next: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl crate::store::CheckpointBlobStore for FailedDeleteStore {
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
        if self
            .fail_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AdapterError::Unavailable(
                "injected uncertain deletion".into(),
            ));
        }
        self.delegate.delete(key).await
    }
}

async fn test_store(prefix: &str) -> (Client, MongoCheckpointObjectStore, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test MongoDB connects");
    let database = format!("cma_checkpoint_{prefix}_{}", uuid::Uuid::new_v4().simple());
    let store = MongoCheckpointObjectStore::open(&client, &database)
        .await
        .unwrap();
    (client, store, database)
}

fn sealed(bytes: &[u8]) -> (String, String) {
    let sha256 = sha256_hex(bytes);
    (format!("home-{sha256}.cma"), sha256)
}

fn upload(bytes: Vec<u8>) -> CheckpointArchiveUpload {
    let (object_reference, sha256) = sealed(&bytes);
    upload_as(bytes, object_reference, sha256)
}

fn upload_as(bytes: Vec<u8>, object_reference: String, sha256: String) -> CheckpointArchiveUpload {
    let size_bytes = bytes.len() as u64;
    let (sender, chunks) = mpsc::channel(2);
    tokio::spawn(async move {
        for chunk in bytes.chunks(192 * 1024) {
            if sender.send(Ok(chunk.to_vec())).await.is_err() {
                return;
            }
        }
    });
    CheckpointArchiveUpload {
        object_reference,
        sha256,
        size_bytes,
        chunks,
    }
}

async fn bytes(download: CheckpointArchiveDownload) -> AdapterResult<Vec<u8>> {
    let mut chunks = download.chunks;
    let mut result = Vec::new();
    while let Some(chunk) = chunks.recv().await {
        result.extend_from_slice(&chunk?);
    }
    Ok(result)
}

async fn round_trip(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    archive: &[u8],
) -> Vec<u8> {
    let authority = store.for_sandbox(sandbox_id).unwrap();
    let receipt = authority.upload(upload(archive.to_vec())).await.unwrap();
    let download = authority.download(&receipt.object_reference).await.unwrap();
    assert_eq!(download.sha256, receipt.sha256);
    assert_eq!(download.size_bytes, receipt.size_bytes);
    bytes(download).await.unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn upload_survives_reopening_and_download_revalidates_bytes() {
    let (client, store, database) = test_store("reopen").await;
    let original = b"durable archive after process replacement".to_vec();
    let authority = store.for_sandbox("sbx_reopen").unwrap();
    let receipt = authority.upload(upload(original.clone())).await.unwrap();
    drop(authority);
    drop(store);

    let reopened = MongoCheckpointObjectStore::open(&client, &database)
        .await
        .unwrap();
    let authority = reopened.for_sandbox("sbx_reopen").unwrap();
    let downloaded = authority.download(&receipt.object_reference).await.unwrap();
    assert_eq!(bytes(downloaded).await.unwrap(), original);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn exact_replay_is_idempotent_and_changed_bytes_do_not_replace_authority() {
    let (client, store, database) = test_store("replay").await;
    let authority = store.for_sandbox("sbx_replay").unwrap();
    let original = b"original sealed checkpoint".to_vec();
    let first = authority.upload(upload(original.clone())).await.unwrap();
    let replay = authority.upload(upload(original.clone())).await.unwrap();
    assert_eq!(replay, first);

    let changed = b"changed! sealed checkpoint".to_vec();
    assert_eq!(changed.len(), original.len());
    let error = authority
        .upload(upload_as(
            changed,
            first.object_reference.clone(),
            first.sha256.clone(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, AdapterError::Rejected(_)));
    let downloaded = authority.download(&first.object_reference).await.unwrap();
    assert_eq!(bytes(downloaded).await.unwrap(), original);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn cleanup_respects_extended_and_legacy_recorded_upload_deadlines() {
    let (client, store, database) = test_store("upload_deadline").await;
    let archive = b"staged upload with a retained deadline";
    let (reference, sha256) = sealed(archive);
    let identity = ArchiveIdentity::from_input(reference, sha256, archive.len() as u64).unwrap();
    let now = DateTime::now();
    let current = ManifestDoc::new("sbx_upload_deadline", &identity, now);
    let mut legacy = ManifestDoc::new("sbx_upload_deadline", &identity, now);
    legacy.cleanup_after = DateTime::from_millis(now.timestamp_millis() + 600_000);
    assert_eq!(
        current.cleanup_after.timestamp_millis() - now.timestamp_millis(),
        1_200_000
    );
    for stage in [&legacy, &current] {
        super::publication::admit(&store, stage).await.unwrap();
        store
            .objects
            .upload(&stage.object_key, upload(archive.to_vec()))
            .await
            .unwrap();
    }
    drop(store);
    let store = MongoCheckpointObjectStore::open(&client, &database)
        .await
        .unwrap();
    for stage in [&legacy, &current] {
        let recorded = store
            .manifests
            .find_one(doc! { "_id": &stage.id })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(super::model::read_manifest(recorded).unwrap(), *stage);
    }

    for elapsed in [599_u64, 600, 1_199, 1_200] {
        store
            .cleanup_due(now.to_system_time() + std::time::Duration::from_secs(elapsed))
            .await
            .unwrap();
        for (stage, deadline) in [(&legacy, 600), (&current, 1_200)] {
            let recorded = store
                .manifests
                .find_one(doc! { "_id": &stage.id })
                .await
                .unwrap();
            let downloaded = store
                .objects
                .download(
                    &stage.object_key,
                    &identity.object_reference,
                    &identity.sha256,
                    identity.size_bytes,
                )
                .await;
            if elapsed < deadline {
                assert_eq!(
                    super::model::read_manifest(recorded.unwrap()).unwrap(),
                    *stage
                );
                assert_eq!(bytes(downloaded.unwrap()).await.unwrap(), archive);
            } else {
                assert!(recorded.is_none());
                assert!(downloaded.is_err());
                assert!(store
                    .tombstones
                    .find_one(doc! { "_id": &stage.id, "object_key": &stage.object_key })
                    .await
                    .unwrap()
                    .is_some());
                assert!(super::publication::publish(&store, stage, &identity)
                    .await
                    .is_err());
            }
        }
    }
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn ownership_is_sandbox_confined_and_purge_does_not_cross_scope() {
    let (client, store, database) = test_store("ownership").await;
    let archive = b"same independently owned archive".to_vec();
    let first = store.for_sandbox("sbx_owner_one").unwrap();
    let second = store.for_sandbox("sbx_owner_two").unwrap();
    let receipt = first.upload(upload(archive.clone())).await.unwrap();

    assert!(matches!(
        second.download(&receipt.object_reference).await,
        Err(AdapterError::Rejected(_))
    ));
    let second_receipt = second.upload(upload(archive.clone())).await.unwrap();
    assert_eq!(second_receipt, receipt);

    store.purge_sandbox("sbx_owner_one").await.unwrap();
    assert!(matches!(
        first.download(&receipt.object_reference).await,
        Err(AdapterError::Rejected(_))
    ));
    let surviving = second
        .download(&second_receipt.object_reference)
        .await
        .unwrap();
    assert_eq!(bytes(surviving).await.unwrap(), archive);
    store.purge_sandbox("sbx_owner_one").await.unwrap();
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn corrupt_manifest_and_external_object_keys_are_rejected() {
    let (client, store, database) = test_store("corrupt").await;
    let authority = store.for_sandbox("sbx_corrupt").unwrap();
    let first = authority
        .upload(upload(b"manifest corruption target".to_vec()))
        .await
        .unwrap();
    store
        .manifests
        .update_one(
            doc! { "sandbox_id": "sbx_corrupt", "object_reference": &first.object_reference },
            doc! { "$set": { "document_schema_version": 2_i64 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        authority.download(&first.object_reference).await,
        Err(AdapterError::Unavailable(_))
    ));

    let second = authority
        .upload(upload(b"external object key corruption target".to_vec()))
        .await
        .unwrap();
    store
        .manifests
        .update_one(
            doc! { "sandbox_id": "sbx_corrupt", "object_reference": &second.object_reference },
            doc! { "$set": { "object_key": "cma/checkpoints/foreign/object.cma" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        authority.download(&second.object_reference).await,
        Err(AdapterError::Unavailable(_))
    ));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn purge_preserves_early_deletion_intent_and_reclaims_a_late_upload() {
    let (client, store, database) = test_store("orphan").await;
    let archive = b"closed but unpublished stage";
    let (object_reference, sha256) = sealed(archive);
    let identity =
        ArchiveIdentity::from_input(object_reference.clone(), sha256, archive.len() as u64)
            .unwrap();
    let manifest = ManifestDoc::new("sbx_orphan", &identity, DateTime::now());
    super::publication::admit(&store, &manifest).await.unwrap();
    store.purge_sandbox("sbx_orphan").await.unwrap();
    assert!(store
        .tombstones
        .find_one(doc! { "_id": &manifest.id })
        .await
        .unwrap()
        .is_some());
    // Simulate a backend finishing an already-admitted upload after the first DELETE/404.
    store
        .objects
        .upload(&manifest.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    assert!(super::publication::publish(&store, &manifest, &identity)
        .await
        .is_err());
    assert!(store
        .for_sandbox("sbx_orphan")
        .unwrap()
        .upload(upload(archive.to_vec()))
        .await
        .is_err());
    let after_client_deadline =
        manifest.cleanup_after.to_system_time() + std::time::Duration::from_secs(3_600);
    store.cleanup_due(after_client_deadline).await.unwrap();
    assert!(store
        .manifests
        .find_one(doc! { "_id": &manifest.id })
        .await
        .unwrap()
        .is_none());
    assert!(store
        .objects
        .download(
            &manifest.object_key,
            &identity.object_reference,
            &identity.sha256,
            identity.size_bytes
        )
        .await
        .is_err());
    assert!(store
        .tombstones
        .find_one(doc! { "_id": &manifest.id })
        .await
        .unwrap()
        .is_some());
    drop(store);
    let store = MongoCheckpointObjectStore::open(&client, &database)
        .await
        .unwrap();
    // Even another late write after a restart and a completed cleanup is owned.
    store
        .objects
        .upload(&manifest.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    store
        .cleanup_due(after_client_deadline + std::time::Duration::from_secs(7_200))
        .await
        .unwrap();
    assert!(store
        .objects
        .download(
            &manifest.object_key,
            &identity.object_reference,
            &identity.sha256,
            identity.size_bytes
        )
        .await
        .is_err());
    store.purge_sandbox("sbx_orphan").await.unwrap();
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn uncertain_external_deletion_retains_durable_work_for_retry() {
    let (client, base, database) = test_store("delete_retry").await;
    let storage = std::sync::Arc::new(FailedDeleteStore {
        delegate: base.objects,
        fail_next: std::sync::atomic::AtomicBool::new(true),
    });
    let store = MongoCheckpointObjectStore::open_with_storage(&client, &database, storage.clone())
        .await
        .unwrap();
    let archive = b"abandoned stage pending external deletion";
    let (reference, sha256) = sealed(archive);
    let identity = ArchiveIdentity::from_input(reference, sha256, archive.len() as u64).unwrap();
    let stage = ManifestDoc::new("sbx_delete_retry", &identity, DateTime::now());
    super::publication::admit(&store, &stage).await.unwrap();
    store
        .objects
        .upload(&stage.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    assert!(store
        .cleanup_due(stage.cleanup_after.to_system_time())
        .await
        .is_err());
    assert!(store
        .manifests
        .find_one(doc! { "_id": &stage.id, "state": "deleting" })
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        store
            .cleanup_due(stage.cleanup_after.to_system_time())
            .await
            .unwrap(),
        1
    );
    assert!(store
        .manifests
        .find_one(doc! { "_id": &stage.id })
        .await
        .unwrap()
        .is_none());
    assert!(store
        .objects
        .download(
            &stage.object_key,
            &identity.object_reference,
            &identity.sha256,
            identity.size_bytes
        )
        .await
        .is_err());
    storage
        .fail_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first_retry = stage.cleanup_after.to_system_time() + std::time::Duration::from_secs(61);
    assert!(store.cleanup_due(first_retry).await.is_err());
    let tombstone = store
        .tombstones
        .find_one(doc! { "_id": &stage.id })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tombstone.get_i64("attempts").unwrap(), 1);
    assert!(
        tombstone
            .get_datetime("next_attempt_at")
            .unwrap()
            .to_system_time()
            > first_retry
    );
    assert_eq!(
        store
            .cleanup_due(first_retry + std::time::Duration::from_secs(3_600))
            .await
            .unwrap(),
        1
    );
    assert!(store
        .tombstones
        .find_one(doc! { "_id": &stage.id })
        .await
        .unwrap()
        .is_some());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_publication_keeps_one_winner_and_cleanup_cannot_delete_it() {
    let (client, store, database) = test_store("winner").await;
    let archive = b"same archive from concurrent writers";
    let (reference, digest) = sealed(archive);
    let identity =
        ArchiveIdentity::from_input(reference.clone(), digest, archive.len() as u64).unwrap();
    let first = ManifestDoc::new("sbx_winner", &identity, DateTime::now());
    let second = ManifestDoc::new("sbx_winner", &identity, DateTime::now());
    for stage in [&first, &second] {
        super::publication::admit(&store, stage).await.unwrap();
        store
            .objects
            .upload(&stage.object_key, upload(archive.to_vec()))
            .await
            .unwrap();
    }
    let (first_result, second_result) = tokio::join!(
        super::publication::publish(&store, &first, &identity),
        super::publication::publish(&store, &second, &identity),
    );
    assert!(first_result.is_ok() || second_result.is_ok());
    let published = store
        .manifests
        .find_one(doc! { "sandbox_id": "sbx_winner", "state": "published" })
        .await
        .unwrap()
        .unwrap();
    let loser = if published.get_str("_id").unwrap() == first.id {
        &second
    } else {
        &first
    };
    assert_eq!(
        store
            .manifests
            .count_documents(doc! { "sandbox_id": "sbx_winner", "state": "published" })
            .await
            .unwrap(),
        1
    );
    store
        .cleanup_due(loser.cleanup_after.to_system_time())
        .await
        .unwrap();
    let authority = store.for_sandbox("sbx_winner").unwrap();
    assert_eq!(
        bytes(authority.download(&reference).await.unwrap())
            .await
            .unwrap(),
        archive
    );
    assert!(store
        .objects
        .download(
            &loser.object_key,
            &reference,
            &identity.sha256,
            identity.size_bytes
        )
        .await
        .is_err());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retired_gate_refuses_replay_and_publication_before_old_manifest_cleanup() {
    let (client, store, database) = test_store("retired").await;
    let archive = b"published before retirement";
    let authority = store.for_sandbox("sbx_retired").unwrap();
    let receipt = authority.upload(upload(archive.to_vec())).await.unwrap();
    let identity = ArchiveIdentity::from_input(
        receipt.object_reference.clone(),
        receipt.sha256,
        receipt.size_bytes,
    )
    .unwrap();
    let stage = ManifestDoc::new("sbx_retired", &identity, DateTime::now());
    super::publication::admit(&store, &stage).await.unwrap();
    store
        .objects
        .upload(&stage.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    store
        .gates
        .update_one(
            doc! { "_id": "sbx_retired" },
            doc! { "$set": { "retired": true }, "$inc": { "serial": 1_i64 } },
        )
        .await
        .unwrap();
    assert!(super::publication::publish(&store, &stage, &identity)
        .await
        .is_err());
    assert!(authority.upload(upload(archive.to_vec())).await.is_err());
    assert_eq!(
        store
            .manifests
            .count_documents(doc! { "sandbox_id": "sbx_retired", "state": "published" })
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn archive_larger_than_bson_document_limit_uses_external_objects() {
    let (client, store, database) = test_store("large").await;
    let mut archive = vec![0_u8; (16 * 1024 * 1024) + 4096];
    for (index, byte) in archive.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    assert_eq!(round_trip(&store, "sbx_large", &archive).await, archive);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publication_keeps_latest_archive_and_prunes_all_parts_of_evicted_generations() {
    let (client, _, database) = test_store("retention").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut current = active_sandbox("sbx_checkpoint_retention");
    store.create_sandbox(current.clone()).await.unwrap();

    let first = stored_checkpoint(&store, &current, 1, b"shared generation").await;
    publish(&store, &mut current, first.clone()).await;
    let second = stored_checkpoint(&store, &current, 2, b"shared generation").await;
    assert_eq!(second.object_reference, first.object_reference);
    publish(&store, &mut current, second).await;
    let shared_key = object_key(&store, &current, &first.object_reference).await;

    let third = stored_checkpoint(&store, &current, 3, b"third generation").await;
    let third_key = object_key(&store, &current, &third.object_reference).await;
    publish(&store, &mut current, third.clone()).await;
    assert_archive_present(&store, &current, &third.object_reference).await;

    let fourth = stored_checkpoint(&store, &current, 4, b"fourth generation").await;
    publish(&store, &mut current, fourth.clone()).await;
    let mut download = layer2_core::sandbox::HierarchyReadStore::read_snapshot_archive(
        &store,
        &current.id,
        &fourth,
    )
    .await
    .unwrap();
    assert_eq!(download.size_bytes, b"fourth generation".len() as u64);
    let mut downloaded = Vec::new();
    while let Some(chunk) = download.chunks.recv().await {
        downloaded.extend(chunk.unwrap());
    }
    assert_eq!(downloaded, b"fourth generation");
    store
        .checkpoint_objects
        .cleanup_due(std::time::SystemTime::now())
        .await
        .unwrap();
    assert_archive_absent(&store, &current, &first.object_reference, &shared_key).await;
    assert_archive_absent(&store, &current, &third.object_reference, &third_key).await;
    assert_archive_present(&store, &current, &fourth.object_reference).await;
    assert_eq!(
        store
            .checkpoint_objects
            .manifests
            .count_documents(doc! { "sandbox_id": current.id.as_str() })
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_publication_reconciles_without_losing_the_new_archive() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("test requires a failpoint-enabled MongoDB replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-checkpoint-retention-{suffix}");
    let database = format!("cma_checkpoint_retention_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut current = active_sandbox("sbx_checkpoint_ambiguous");
    store.create_sandbox(current.clone()).await.unwrap();

    let first = stored_checkpoint(&store, &current, 1, b"first generation").await;
    publish(&store, &mut current, first.clone()).await;
    let first_key = object_key(&store, &current, &first.object_reference).await;
    let second = stored_checkpoint(&store, &current, 2, b"second generation").await;
    publish(&store, &mut current, second.clone()).await;
    let second_key = object_key(&store, &current, &second.object_reference).await;
    let third = stored_checkpoint(&store, &current, 3, b"just published generation").await;

    configure_commit_ambiguity(&admin, &app_name).await;
    let result = publish_result(&store, &current, third.clone()).await;
    disable_failpoint(&admin).await;
    current = result.unwrap();

    store
        .checkpoint_objects
        .cleanup_due(std::time::SystemTime::now())
        .await
        .unwrap();
    assert_archive_absent(&store, &current, &first.object_reference, &first_key).await;
    assert_archive_absent(&store, &current, &second.object_reference, &second_key).await;
    assert_archive_present(&store, &current, &third.object_reference).await;
    assert_eq!(current.lifecycle.checkpoints.latest, Some(third));
    assert!(current.lifecycle.checkpoints.previous.is_none());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_external_stage_admission_and_publication_reconcile_exactly() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-external-stage-{suffix}");
    let database = format!("cma_checkpoint_stage_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoCheckpointObjectStore::open(&client, &database)
        .await
        .unwrap();
    let archive = b"exact stage after lost commit acknowledgement";
    let (reference, sha256) = sealed(archive);
    let identity =
        ArchiveIdentity::from_input(reference.clone(), sha256, archive.len() as u64).unwrap();
    let stage = ManifestDoc::new("sbx_ambiguous_stage", &identity, DateTime::now());
    configure_commit_ambiguity(&admin, &app_name).await;
    let admitted = super::publication::admit(&store, &stage).await;
    disable_failpoint(&admin).await;
    admitted.unwrap();
    store
        .objects
        .upload(&stage.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    configure_commit_ambiguity(&admin, &app_name).await;
    let published = super::publication::publish(&store, &stage, &identity).await;
    disable_failpoint(&admin).await;
    published.unwrap();
    assert_eq!(
        store
            .manifests
            .count_documents(doc! { "sandbox_id": "sbx_ambiguous_stage", "state": "published" })
            .await
            .unwrap(),
        1
    );
    let authority = store.for_sandbox("sbx_ambiguous_stage").unwrap();
    assert_eq!(
        bytes(authority.download(&reference).await.unwrap())
            .await
            .unwrap(),
        archive
    );
    let uncertain = ManifestDoc::new("sbx_ambiguous_stage", &identity, DateTime::now());
    super::publication::admit(&store, &uncertain).await.unwrap();
    store
        .objects
        .upload(&uncertain.object_key, upload(archive.to_vec()))
        .await
        .unwrap();
    configure_commit_ambiguity(&admin, &app_name).await;
    let purged = store.purge_sandbox("sbx_ambiguous_stage").await;
    disable_failpoint(&admin).await;
    purged.unwrap();
    assert!(store
        .tombstones
        .find_one(doc! { "_id": &uncertain.id })
        .await
        .unwrap()
        .is_some());
    assert!(store
        .manifests
        .find_one(doc! { "_id": &uncertain.id })
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}

fn active_sandbox(id: &str) -> Sandbox {
    let mut value = sandbox(id);
    value.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{id}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: "2026-09-05T00:00:00Z".into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, "2026-09-05T00:00:00Z".into());
    value
}

fn retired_sandbox(id: &str) -> Sandbox {
    let mut value = sandbox(id);
    value.status = layer2_core::sandbox::SandboxStatus::Retired;
    value.runtime = None;
    value.credential.current = None;
    value.credential.delivered = false;
    value.retired_at = Some("2026-09-05T00:00:00Z".into());
    value.purge_after = Some("2026-10-05T00:00:00Z".into());
    value.work =
        SandboxWorkState::new(SandboxWorkStep::WaitForPurge, "2026-10-05T00:00:00Z".into());
    value
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cleanup_reclaims_legacy_deleted_snapshots_without_purging_retained_records() {
    let (client, _, database) = test_store("backfill").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let retired = retired_sandbox("sbx_legacy_deleted");
    let active = active_sandbox("sbx_keep_live");
    store.create_sandbox(retired.clone()).await.unwrap();
    store.create_sandbox(active.clone()).await.unwrap();
    let first = stored_checkpoint(&store, &retired, 1, b"old snapshot").await;
    let second = stored_checkpoint(&store, &retired, 2, b"latest deleted snapshot").await;
    let live = stored_checkpoint(&store, &active, 1, b"live snapshot").await;
    let old_key = object_key(&store, &retired, &first.object_reference).await;
    let latest_key = object_key(&store, &retired, &second.object_reference).await;
    round_trip(
        &store.checkpoint_objects,
        "sbx_missing_parent",
        b"unclassified archive",
    )
    .await;
    let now = std::time::SystemTime::now();
    assert_eq!(store.checkpoint_objects.cleanup_due(now).await.unwrap(), 3);
    assert_archive_absent(&store, &retired, &first.object_reference, &old_key).await;
    assert_archive_absent(&store, &retired, &second.object_reference, &latest_key).await;
    assert_archive_present(&store, &active, &live.object_reference).await;
    assert_eq!(
        store.get_sandbox(&retired.id).await.unwrap(),
        Some(retired.clone())
    );
    assert_eq!(store.get_sandbox(&active.id).await.unwrap(), Some(active));
    assert!(store
        .checkpoint_objects
        .gates
        .find_one(doc! { "_id": retired.id.as_str(), "retired": true })
        .await
        .unwrap()
        .is_some());
    assert!(store
        .checkpoint_objects
        .gates
        .find_one(doc! { "_id": "sbx_missing_parent", "retired": true })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        round_trip(
            &store.checkpoint_objects,
            "sbx_missing_parent",
            b"unclassified archive"
        )
        .await,
        b"unclassified archive"
    );
    assert!(store
        .checkpoint_objects
        .for_sandbox(retired.id.as_str())
        .unwrap()
        .upload(upload(b"late publication".to_vec()))
        .await
        .is_err());
    assert_eq!(store.checkpoint_objects.cleanup_due(now).await.unwrap(), 0);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_snapshot_cleanup_refuses_inconsistent_retired_authority() {
    let (client, _, database) = test_store("retired_corrupt").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let retired = retired_sandbox("sbx_invalid_retired");
    store.create_sandbox(retired.clone()).await.unwrap();
    let checkpoint = stored_checkpoint(&store, &retired, 1, b"preserve on corrupt authority").await;
    store
        .sandboxes
        .update_one(
            doc! { "_id": retired.id.as_str() },
            doc! { "$set": { "name": "mismatched name" } },
        )
        .await
        .unwrap();
    assert!(store
        .checkpoint_objects
        .cleanup_due(std::time::SystemTime::now())
        .await
        .is_err());
    assert_archive_present(&store, &retired, &checkpoint.object_reference).await;
    assert!(store
        .checkpoint_objects
        .gates
        .find_one(doc! { "_id": retired.id.as_str(), "retired": true })
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}

async fn stored_checkpoint(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    epoch: u64,
    bytes: &[u8],
) -> SandboxCheckpointRef {
    let receipt = store
        .checkpoint_objects
        .for_sandbox(sandbox.id.as_str())
        .unwrap()
        .upload(upload(bytes.to_vec()))
        .await
        .unwrap();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
            writers: WriterState::Held,
        },
        &[],
        "2026-09-05T00:00:00Z",
    )
    .unwrap()
}

async fn publish(
    store: &MongoSandboxStore,
    current: &mut Sandbox,
    checkpoint: SandboxCheckpointRef,
) {
    *current = publish_result(store, current, checkpoint).await.unwrap();
}

async fn publish_result(
    store: &MongoSandboxStore,
    current: &Sandbox,
    checkpoint: SandboxCheckpointRef,
) -> Result<Sandbox, layer2_core::sandbox::store::SandboxStoreError> {
    let mut next = current.clone();
    next.publish_opportunistic_checkpoint(checkpoint.clone(), "2026-09-05T00:00:00Z".into())
        .unwrap();
    store
        .publish_sandbox_checkpoint(SandboxCheckpointMutation {
            guard: SandboxMutationGuard::capture(current),
            agents: Vec::new(),
            nonblocking: false,
            checkpoint,
            sandbox: next.clone(),
            appended_at: "2026-09-05T00:00:00Z".into(),
        })
        .await?;
    Ok(next)
}

async fn object_key(store: &MongoSandboxStore, sandbox: &Sandbox, reference: &str) -> String {
    store
        .checkpoint_objects
        .manifests
        .find_one(doc! {
            "sandbox_id": sandbox.id.as_str(),
            "object_reference": reference,
            "state": "published",
        })
        .await
        .unwrap()
        .unwrap()
        .get_str("object_key")
        .unwrap()
        .to_owned()
}

async fn assert_archive_present(store: &MongoSandboxStore, sandbox: &Sandbox, reference: &str) {
    let authority = store
        .checkpoint_objects
        .for_sandbox(sandbox.id.as_str())
        .unwrap();
    assert!(!bytes(authority.download(reference).await.unwrap())
        .await
        .unwrap()
        .is_empty());
}

async fn assert_archive_absent(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    reference: &str,
    key: &str,
) {
    assert!(store
        .checkpoint_objects
        .manifests
        .find_one(doc! {
            "sandbox_id": sandbox.id.as_str(),
            "object_reference": reference,
        })
        .await
        .unwrap()
        .is_none());
    assert!(store
        .checkpoint_objects
        .objects
        .download(key, reference, "unused", 1)
        .await
        .is_err());
}

async fn configure_commit_ambiguity(admin: &Client, app_name: &str) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous checkpoint retention commit",
                },
            },
        })
        .await
        .unwrap();
}

async fn disable_failpoint(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");
}

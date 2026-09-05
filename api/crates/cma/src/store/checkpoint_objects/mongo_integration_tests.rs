use futures_util::io::AsyncWriteExt;
use layer2_core::sandbox::store::{
    SandboxCheckpointMutation, SandboxLifecycleExecutionStore, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxWorkState, SandboxWorkStep,
};
use layer3_adapter::hosted::{CheckpointArchiveDownload, CheckpointArchiveUpload};
use layer3_adapter::{AdapterError, CheckpointReceipt, Result as AdapterResult, WriterState};
use mongodb::bson::{doc, Bson};
use mongodb::options::ClientOptions;
use mongodb::Client;
use tokio::sync::mpsc;

use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::sandbox;

use super::model::{
    metadata_document, sha256_hex, ArchiveIdentity, ManifestDoc, GRIDFS_CHUNK_BYTES,
};
use super::MongoCheckpointObjectStore;

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
async fn corrupt_manifest_and_gridfs_metadata_are_rejected() {
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
        .upload(upload(b"GridFS metadata corruption target".to_vec()))
        .await
        .unwrap();
    let second_manifest = store
        .manifests
        .find_one(doc! {
            "sandbox_id": "sbx_corrupt",
            "object_reference": &second.object_reference,
        })
        .await
        .unwrap()
        .unwrap();
    let gridfs_id = second_manifest.get_object_id("gridfs_id").unwrap();
    store
        .files
        .update_one(
            doc! { "_id": gridfs_id },
            doc! { "$set": { "metadata.sandbox_id": "sbx_foreign" } },
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
async fn purge_reclaims_closed_unpublished_gridfs_stage() {
    let (client, store, database) = test_store("orphan").await;
    let archive = b"closed but unpublished stage";
    let (object_reference, sha256) = sealed(archive);
    let identity =
        ArchiveIdentity::from_input(object_reference.clone(), sha256, archive.len() as u64)
            .unwrap();
    let manifest = ManifestDoc::new("sbx_orphan", &identity, mongodb::bson::oid::ObjectId::new());
    let mut stream = store
        .bucket
        .open_upload_stream(&object_reference)
        .id(Bson::ObjectId(manifest.gridfs_id))
        .chunk_size_bytes(GRIDFS_CHUNK_BYTES)
        .metadata(metadata_document(&manifest).unwrap())
        .await
        .unwrap();
    stream.write_all(archive).await.unwrap();
    stream.close().await.unwrap();

    store.purge_sandbox("sbx_orphan").await.unwrap();
    assert!(store
        .files
        .find_one(doc! { "_id": manifest.gridfs_id })
        .await
        .unwrap()
        .is_none());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn archive_larger_than_bson_document_limit_streams_through_gridfs() {
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
async fn publication_keeps_two_archives_and_prunes_all_parts_of_the_evicted_one() {
    let (client, _, database) = test_store("retention").await;
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut current = active_sandbox("sbx_checkpoint_retention");
    store.create_sandbox(current.clone()).await.unwrap();

    let first = stored_checkpoint(&store, &current, 1, b"shared generation").await;
    publish(&store, &mut current, first.clone()).await;
    let second = stored_checkpoint(&store, &current, 2, b"shared generation").await;
    assert_eq!(second.object_reference, first.object_reference);
    publish(&store, &mut current, second).await;
    let shared_id = gridfs_id(&store, &current, &first.object_reference).await;

    let third = stored_checkpoint(&store, &current, 3, b"third generation").await;
    publish(&store, &mut current, third.clone()).await;
    assert_archive_present(&store, &current, &first.object_reference).await;

    let fourth = stored_checkpoint(&store, &current, 4, b"fourth generation").await;
    publish(&store, &mut current, fourth.clone()).await;
    assert_archive_absent(&store, &current, &first.object_reference, shared_id).await;
    assert_archive_present(&store, &current, &third.object_reference).await;
    assert_archive_present(&store, &current, &fourth.object_reference).await;
    assert_eq!(
        store
            .checkpoint_objects
            .manifests
            .count_documents(doc! { "sandbox_id": current.id.as_str() })
            .await
            .unwrap(),
        2
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
    let second = stored_checkpoint(&store, &current, 2, b"second generation").await;
    publish(&store, &mut current, second.clone()).await;
    let first_id = gridfs_id(&store, &current, &first.object_reference).await;
    let third = stored_checkpoint(&store, &current, 3, b"just published generation").await;

    configure_commit_ambiguity(&admin, &app_name).await;
    let result = publish_result(&store, &current, third.clone()).await;
    disable_failpoint(&admin).await;
    current = result.unwrap();

    assert_archive_absent(&store, &current, &first.object_reference, first_id).await;
    assert_archive_present(&store, &current, &second.object_reference).await;
    assert_archive_present(&store, &current, &third.object_reference).await;
    assert_eq!(current.lifecycle.checkpoints.latest, Some(third));
    assert_eq!(current.lifecycle.checkpoints.previous, Some(second));
    client.database(&database).drop().await.unwrap();
}

fn active_sandbox(id: &str) -> Sandbox {
    let mut value = sandbox(id);
    value.runtime = Some(RuntimeBinding {
        runtime_id: format!("runtime-{id}"),
        runtime_version: "image-v1".into(),
        bound_at: "2026-09-05T00:00:00Z".into(),
    });
    value.work = SandboxWorkState::new(SandboxWorkStep::None, "2026-09-05T00:00:00Z".into());
    value
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
            checkpoint,
            sandbox: next.clone(),
            appended_at: "2026-09-05T00:00:00Z".into(),
        })
        .await?;
    Ok(next)
}

async fn gridfs_id(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    reference: &str,
) -> mongodb::bson::oid::ObjectId {
    store
        .checkpoint_objects
        .manifests
        .find_one(doc! {
            "sandbox_id": sandbox.id.as_str(),
            "object_reference": reference,
        })
        .await
        .unwrap()
        .unwrap()
        .get_object_id("gridfs_id")
        .unwrap()
}

async fn assert_archive_present(store: &MongoSandboxStore, sandbox: &Sandbox, reference: &str) {
    let id = gridfs_id(store, sandbox, reference).await;
    assert!(store
        .checkpoint_objects
        .files
        .find_one(doc! { "_id": id })
        .await
        .unwrap()
        .is_some());
    assert!(
        store
            .checkpoint_objects
            .chunks
            .count_documents(doc! { "files_id": id })
            .await
            .unwrap()
            > 0
    );
}

async fn assert_archive_absent(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    reference: &str,
    id: mongodb::bson::oid::ObjectId,
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
        .files
        .find_one(doc! { "_id": id })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .checkpoint_objects
            .chunks
            .count_documents(doc! { "files_id": id })
            .await
            .unwrap(),
        0
    );
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

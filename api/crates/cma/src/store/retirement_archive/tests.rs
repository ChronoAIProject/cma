use super::*;
use crate::retirement_archive::{
    export,
    tests::{qualify, request, Custody, Temp},
    verify,
};
use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind, SandboxHome, SandboxWorkStep};
use layer3_adapter::{permanent_custody::PermanentCustody, HomeVolumePolicy, HomeVolumeReceipt};

#[path = "backup_compatibility_tests.rs"]
mod backup_compatibility_tests;
#[path = "canary_compatibility_tests.rs"]
mod canary_compatibility_tests;
#[path = "census_tests.rs"]
mod census_tests;
pub(in crate::store) use census_tests::prepare_source;
#[path = "commit_test_support.rs"]
mod commit_support;
#[path = "completion_tests.rs"]
mod completion_tests;
#[path = "schema_compatibility_tests.rs"]
mod schema_compatibility_tests;

pub(super) async fn fixture() -> (Temp, ArchiveStore, Request, Vec<u8>) {
    let client = Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo required"),
    )
    .await
    .unwrap();
    let database = client.database(&format!(
        "retirement_archive_{}",
        uuid::Uuid::new_v4().simple()
    ));
    fixture_on(database).await
}

async fn fixture_on(database: Database) -> (Temp, ArchiveStore, Request, Vec<u8>) {
    let client = database.client().clone();
    let temp = Temp::new();
    let mut request = request(&temp);
    request.sandbox_id = "sbx_retirement_fixture".into();
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(&request.sandbox_id);
    sandbox.status = SandboxStatus::Retiring;
    sandbox.work.step = SandboxWorkStep::StopAgents;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "volume".into(),
        storage_generation: "generation".into(),
    })
    .unwrap();
    request.home_identity = home.attachment().unwrap().identity().unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.validate().unwrap();
    let source = crate::store::sandboxes::sandbox_document(&sandbox, 1).unwrap();
    let event = crate::store::sandbox_events::event_document(&SandboxEvent {
        sandbox_id: sandbox.id,
        seq: 1,
        appended_at: layer2_core::now_rfc3339(),
        kind: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Retiring,
        },
    })
    .unwrap();
    request.source_sha256 = digest(&mongodb::bson::serialize_to_vec(&source).unwrap());
    request.retirement_intent_sha256 = digest(&mongodb::bson::serialize_to_vec(&event).unwrap());
    qualify(&temp, &mut request);
    let bytes = serde_json::to_vec(&request).unwrap();
    database
        .collection::<Document>("sandboxes")
        .insert_one(source)
        .await
        .unwrap();
    database
        .collection::<Document>("sandbox_events")
        .insert_one(event)
        .await
        .unwrap();
    (temp, ArchiveStore { client, database }, request, bytes)
}

pub(in crate::store) async fn verify_legal_consent_census(admin: &Database, runtime: &Database) {
    let (_temp, admin_store, request, bytes) = fixture_on(admin.clone()).await;
    admin_store.admit(&request, &bytes).await.unwrap();
    let store = ArchiveStore {
        client: runtime.client().clone(),
        database: runtime.clone(),
    };
    let custody = Custody::default();
    let current = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    // These are the exact schema and names accepted by the predecessor.
    assert_eq!(
        current.schema_sha256,
        super::schema::PRE_LEGAL_CONSENT_SHA256
    );
    let names: std::collections::BTreeSet<_> = current
        .database
        .iter()
        .map(|row| row.collection.as_str())
        .collect();
    let predecessor: std::collections::BTreeSet<_> = super::schema::PRE_LEGAL_CONSENT_COLLECTIONS
        .iter()
        .copied()
        .collect();
    assert_eq!(names, predecessor);
    assert_eq!(names.len(), current.database.len());
    assert!(super::matches_current(&current, current.clone()).unwrap());
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn durable_hold_refuses_stale_workers_saves_and_unverified_completion() {
    let (_temp, store, request, bytes) = fixture().await;
    let ordinary =
        crate::store::sandbox_mongo::MongoSandboxStore::open(&store.client, store.database.name())
            .await
            .unwrap();
    let original = read_sandbox(&store.inspect(&request.sandbox_id).await.unwrap()).unwrap();
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    store.admit(&request, &bytes).await.unwrap();
    assert_eq!(held, store.inspect(&request.sandbox_id).await.unwrap());
    let parent = read_sandbox(&held).unwrap();
    assert!(parent.retirement_archive_pending());
    let mut stale = original.clone();
    stale.revision += 1;
    assert!(ordinary
        .save_sandbox(original.revision, stale)
        .await
        .is_err());
    let mut bypass = parent.clone();
    bypass.lifecycle.retirement_archive = None;
    bypass.revision += 1;
    assert!(ordinary
        .save_sandbox(parent.revision, bypass)
        .await
        .is_err());
    assert!(ordinary
        .claim_sandbox_work(
            &parent.id,
            "fixture",
            layer2_core::now_rfc3339(),
            "2099-01-01T00:00:00Z".into()
        )
        .await
        .unwrap()
        .is_none());
    assert!(ordinary
        .purge_sandbox(&parent.id, parent.revision, "2099-01-01T00:00:00Z")
        .await
        .is_err());
    let receipt = RetirementArchiveReceipt {
        manifest_sha256: "a".repeat(64),
        custody_sha256: "b".repeat(64),
        verification_sha256: "c".repeat(64),
    };
    assert!(store.complete(&request, &bytes, receipt).await.is_err());
    assert_eq!(held, store.inspect(&request.sandbox_id).await.unwrap());
    store.database.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn full_archive_reconstructs_home_conversations_and_ledgers_before_gate_release() {
    let (temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let (_, workspace, worktree, mut agent) =
        crate::store::sandbox_mongo_integration_tests::ledger_retirement_tests::hierarchy();
    agent.conversation.last_seq = 2;
    for (offset, kind) in [
        layer2_core::sandbox::AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        layer2_core::sandbox::AgentLogKind::OwnerMessage {
            content: "retained conversation".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let entry = layer2_core::sandbox::AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: offset as u64 + 1,
            appended_at: layer2_core::now_rfc3339(),
            kind,
        };
        store
            .database
            .collection::<Document>("agent_log")
            .insert_one(crate::store::agent_log::log_document(&entry).unwrap())
            .await
            .unwrap();
    }
    for (name, document) in [
        (
            "workspaces",
            crate::store::workspaces::workspace_document(&workspace).unwrap(),
        ),
        (
            "worktrees",
            crate::store::worktrees::worktree_document(&worktree).unwrap(),
        ),
        (
            "agents",
            crate::store::agents::agent_document(&agent).unwrap(),
        ),
    ] {
        store
            .database
            .collection::<Document>(name)
            .insert_one(document)
            .await
            .unwrap();
    }
    for (collection, document) in [
        (
            "agent_artifacts",
            doc! {"_id":"artifact1","document_schema_version":1_i64,"agent_id":"agt_atomic","content_base64":"cHJlc2VydmVk"},
        ),
        (
            "agent_log",
            crate::store::agent_log::log_document(&layer2_core::sandbox::AgentLogEntry {
                agent_id: layer2_core::AgentId("agt_unrelated".into()),
                seq: 1,
                appended_at: layer2_core::now_rfc3339(),
                kind: layer2_core::sandbox::AgentLogKind::Created {
                    workspace_id: layer2_core::WorkspaceId("wks_unrelated".into()),
                    worktree_id: layer2_core::WorktreeId("wtr_unrelated".into()),
                },
            })
            .unwrap(),
        ),
    ] {
        store
            .database
            .collection::<Document>(collection)
            .insert_one(document)
            .await
            .unwrap();
    }
    let custody = Custody::default();
    let manifest = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let expected = verify::load(&custody, &manifest, &request, &bytes)
        .await
        .unwrap();
    assert_eq!(
        expected
            .database
            .iter()
            .find(|r| r.collection == "agent_log")
            .unwrap()
            .documents,
        2
    );
    let verification = verify::restore(
        &custody,
        &manifest,
        &request,
        &bytes,
        &temp.0.join("restore"),
    )
    .await
    .unwrap();
    assert_eq!(verification.files, 6);
    assert!(verification.documents >= 5);
    let restore = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&verification).unwrap(),
        )
        .await
        .unwrap();
    store
        .retain_restore(&request, &bytes, &restore)
        .await
        .unwrap();
    let receipt = RetirementArchiveReceipt {
        manifest_sha256: manifest.sha256,
        custody_sha256: verification.custody_sha256,
        verification_sha256: restore.sha256,
    };
    let mut bad = receipt.clone();
    bad.verification_sha256 = "a".repeat(64);
    assert!(store.complete(&request, &bytes, bad).await.is_err());
    let (archive, observed) = commit_support::begin(&store).await;
    let result = archive.complete(&request, &bytes, receipt.clone()).await;
    commit_support::finish(&store, observed).await;
    result.unwrap();
    let completed = store.inspect(&request.sandbox_id).await.unwrap();
    store.complete(&request, &bytes, receipt).await.unwrap();
    assert_eq!(completed, store.inspect(&request.sandbox_id).await.unwrap());
    assert!(!read_sandbox(&completed)
        .unwrap()
        .retirement_archive_pending());
    let ordinary =
        crate::store::sandbox_mongo::MongoSandboxStore::open(&store.client, store.database.name())
            .await
            .unwrap();
    let parent = read_sandbox(&completed).unwrap();
    let resumed = ordinary
        .claim_sandbox_work(
            &parent.id,
            "fixture",
            layer2_core::now_rfc3339(),
            "2099-01-01T00:00:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.status, SandboxStatus::Retiring);
    assert_eq!(resumed.work.step, SandboxWorkStep::StopAgents);
    assert_eq!(
        resumed.lifecycle.retirement_archive,
        parent.lifecycle.retirement_archive
    );
    store.database.drop().await.unwrap();
    for root in &request.roots {
        std::fs::remove_dir_all(&root.path).unwrap();
    }
    assert!(verify::restore(
        &custody,
        &verification.manifest,
        &request,
        &bytes,
        &temp.0.join("after-source")
    )
    .await
    .is_ok());
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn wrong_original_changed_source_and_missing_custody_never_release_hold() {
    let (temp, store, mut request, bytes) = fixture().await;
    let wrong = request.source_sha256.clone();
    request.source_sha256 = "f".repeat(64);
    assert!(store
        .admit(&request, &serde_json::to_vec(&request).unwrap())
        .await
        .is_err());
    request.source_sha256 = wrong;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let receipt = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    std::fs::write(request.roots[0].path.join("data"), b"changed source").unwrap();
    assert!(export::publish(&store, &request, &bytes, &custody, None)
        .await
        .is_err());
    custody.0.lock().unwrap().remove(&receipt.key);
    assert!(verify::restore(
        &custody,
        &receipt,
        &request,
        &bytes,
        &temp.0.join("restore")
    )
    .await
    .is_err());
    assert!(
        read_sandbox(&store.inspect(&request.sandbox_id).await.unwrap())
            .unwrap()
            .retirement_archive_pending()
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn checkpoint_tombstone_retry_is_held_and_wrong_epoch_cannot_replace_original() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let prefix = format!(
        "cma/checkpoints/{}/fixture.cma",
        digest(request.sandbox_id.as_bytes())
    );
    assert!(holds_checkpoint_key(&store.database, &prefix)
        .await
        .unwrap());
    assert!(
        !holds_checkpoint_key(&store.database, "cma/checkpoints/unrelated/fixture.cma")
            .await
            .unwrap()
    );
    let mut replacement = request.clone();
    replacement.epoch += 1;
    assert!(store
        .admit(&replacement, &serde_json::to_vec(&replacement).unwrap())
        .await
        .is_err());
    let mut replacement = request.clone();
    replacement.operation_id = uuid::Uuid::new_v4().to_string();
    assert!(store
        .admit(&replacement, &serde_json::to_vec(&replacement).unwrap())
        .await
        .is_err());
    // A direct out-of-contract mutation cannot be hidden by retaining the marker.
    store
        .parents()
        .update_one(
            doc! {"_id":&request.sandbox_id},
            doc! {"$set":{"record.name":"changed"}},
        )
        .await
        .unwrap();
    assert!(store.original(&request, &bytes).await.is_err());
    store.database.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn unsupported_external_objects_refuse_without_certifying_metadata() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    store
        .database
        .collection::<Document>("checkpoint_objects")
        .insert_one(
            doc! {"_id":"external","sandbox_id":&request.sandbox_id,"object_key":"not-captured"},
        )
        .await
        .unwrap();
    assert!(
        export::publish(&store, &request, &bytes, &Custody::default(), None)
            .await
            .is_err()
    );
    assert!(store.manifest(&request, &bytes).await.is_err());
    assert!(
        read_sandbox(&store.inspect(&request.sandbox_id).await.unwrap())
            .unwrap()
            .retirement_archive_pending()
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_hold_commit_reconciles_same_original_without_duplicate_usage() {
    let (_temp, store, request, bytes) = fixture().await;
    let (archive, observed) = commit_support::begin(&store).await;
    let result = archive.admit(&request, &bytes).await;
    commit_support::finish(&store, observed).await;
    result.unwrap();
    let accepted = store.inspect(&request.sandbox_id).await.unwrap();
    let restarted = ArchiveStore {
        client: store.client.clone(),
        database: store.database.clone(),
    };
    restarted.admit(&request, &bytes).await.unwrap();
    assert_eq!(accepted, store.inspect(&request.sandbox_id).await.unwrap());
    assert_eq!(
        store
            .database
            .collection::<Document>(super::super::sandbox_usage::EVENTS)
            .count_documents(doc! {"sandbox_id":&request.sandbox_id})
            .await
            .unwrap(),
        1
    );
    store.database.drop().await.unwrap();
}

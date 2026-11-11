use super::*;
use layer2_core::sandbox::ledger_collection_artifact::LedgerCollectionArtifactReferenceV1;
use layer2_core::sandbox::store::ledger_collection::checkpoint::{
    LedgerCollectionCheckpointMutation as M, LedgerCollectionCheckpointStore,
    LedgerCollectionCheckpointV2, LedgerCollectionFinalAcknowledgement,
};

fn checkpoint() -> LedgerCollectionCheckpointV2 {
    LedgerCollectionCheckpointV2::new("a".repeat(64), 1, 7, "b".repeat(64), 10, 20).unwrap()
}
fn reference() -> LedgerCollectionArtifactReferenceV1 {
    LedgerCollectionArtifactReferenceV1 {
        format: "runtime-ledger-collection-reference-v1".into(),
        archive_id: uuid::Uuid::from_u128(1).to_string(),
        archive_version: 3,
        operation_id: uuid::Uuid::from_u128(2).to_string(),
        group_key: "1".repeat(64),
        binding_sha256: "a".repeat(64),
        plan_sha256: "c".repeat(64),
        artifacts_sha256: "d".repeat(64),
        artifact_lengths: [100, 100, 100],
    }
}
fn page() -> LedgerCollectionPageProgress {
    LedgerCollectionPageProgress {
        page_sha256: "e".repeat(64),
        active_revision: 7,
        cursor: None,
        next_cursor: None,
        complete: true,
        remaining_groups: vec!["1".repeat(64)],
    }
}
fn selected() -> LedgerCollectionPending {
    LedgerCollectionPending {
        operation_id: reference().operation_id,
        group_key: "1".repeat(64),
        request_sha256: "2".repeat(64),
        observation_sha256: "3".repeat(64),
        source_sha256: "4".repeat(64),
        phase: LedgerCollectionPhase::Selected,
        receipt_sha256: None,
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn checkpoint_mongo_takeover_save_and_atomic_ack_settlement() {
    let (client, name) = database("checkpoint_takeover").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_checkpoint_takeover");
    first.create_sandbox(parent.clone()).await.unwrap();
    first
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint())
        .await
        .unwrap();
    first
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint())
        .await
        .unwrap();
    assert!(
        first.ledger_collection_progress(&parent.id).await.is_err(),
        "old reader refuses B2"
    );
    let state = first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            1,
            M::PinPage {
                fence: 1,
                now: 11,
                page: page(),
            },
        )
        .await
        .unwrap();
    let state = first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::RecordReference {
                fence: 1,
                now: 11,
                reference: reference(),
            },
        )
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .ledger_collection_checkpoint(&parent.id)
            .await
            .unwrap(),
        Some(state.clone())
    );
    let (a, b) = tokio::join!(
        first.update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::Claim {
                holder_sha256: "e".repeat(64),
                now: 20,
                expires_at: 30
            }
        ),
        second.update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::Claim {
                holder_sha256: "f".repeat(64),
                now: 20,
                expires_at: 30
            }
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let state = a.or(b).unwrap();
    assert_eq!(state.accepted_artifact, Some(reference()));
    assert!(first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::Refresh {
                fence: 2,
                now: 21,
                active_revision: 8
            }
        )
        .await
        .is_err());
    let state = first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::Select {
                fence: 2,
                now: 21,
                pending: selected(),
            },
        )
        .await
        .unwrap();
    let acknowledgement = LedgerCollectionFinalAcknowledgement {
        operation_id: reference().operation_id,
        binding_sha256: "a".repeat(64),
        plan_sha256: "c".repeat(64),
        group_key: "1".repeat(64),
        source_sha256: "4".repeat(64),
        observation_sha256: "3".repeat(64),
        before_revision: 7,
        after_revision: 8,
        receipt_sha256: "5".repeat(64),
    };
    let original = first
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    for after_revision in [7, 9, u64::MAX] {
        let mut changed = acknowledgement.clone();
        changed.after_revision = after_revision;
        assert!(first
            .update_ledger_collection_checkpoint(
                &parent.id,
                parent.revision,
                state.progress.revision,
                M::Acknowledge {
                    fence: 2,
                    now: 21,
                    acknowledgement: changed
                }
            )
            .await
            .is_err());
        assert_eq!(
            first
                .sandboxes
                .find_one(doc! {"_id":parent.id.as_str()})
                .await
                .unwrap()
                .unwrap(),
            original,
            "invalid acknowledgement writes nothing"
        );
    }
    let state = first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::Acknowledge {
                fence: 2,
                now: 21,
                acknowledgement,
            },
        )
        .await
        .unwrap();
    let before = first
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    parent.revision += 1;
    parent.updated_at = MINUTE.into();
    first
        .save_sandbox(parent.revision - 1, parent.clone())
        .await
        .unwrap();
    let after = first
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.get("ledger_collection_v1"),
        after.get("ledger_collection_v1")
    );
    let settle = M::SettleAcknowledged {
        fence: 2,
        now: 21,
        operation_id: reference().operation_id,
        receipt_sha256: "5".repeat(64),
    };
    assert!(first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision - 1,
            state.progress.revision,
            settle.clone()
        )
        .await
        .is_err());
    let done = first
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            settle,
        )
        .await
        .unwrap();
    assert_eq!(done.settled_after, Some("1".repeat(64)));
    assert_eq!(done.progress.active_revision, 8);
    assert!(done.accepted_artifact.is_none() && done.progress.pending.is_none());
    assert!(done.progress.page.is_none() && done.progress.cursor.is_none());
    assert_eq!(
        reopened
            .ledger_collection_checkpoint(&parent.id)
            .await
            .unwrap(),
        Some(done)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn checkpoint_mongo_refuses_legacy_and_malformed_markers_without_writes() {
    let (client, name) = database("checkpoint_compatibility").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for case in 0..4 {
        let mut parent = sandbox(&format!("sbx_checkpoint_compatibility_{case}"));
        parent.name = format!("checkpoint-compatibility-{case}");
        store.create_sandbox(parent.clone()).await.unwrap();
        let marker = match case {
            0 => mongodb::bson::serialize_to_bson(&progress()).unwrap(),
            1 => Bson::Null,
            2 => Bson::Document(doc! {"format":"future-format"}),
            _ => {
                let mut invalid = checkpoint();
                invalid.progress.revision = 0;
                mongodb::bson::serialize_to_bson(&invalid).unwrap()
            }
        };
        store
            .sandboxes
            .update_one(
                doc! {"_id":parent.id.as_str()},
                doc! {"$set":{"ledger_collection_v1":marker}},
            )
            .await
            .unwrap();
        let before = store
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(store
            .ledger_collection_checkpoint(&parent.id)
            .await
            .is_err());
        assert!(store
            .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint())
            .await
            .is_err());
        assert!(store
            .update_ledger_collection_checkpoint(
                &parent.id,
                parent.revision,
                1,
                M::Claim {
                    holder_sha256: "f".repeat(64),
                    now: 20,
                    expires_at: 30
                }
            )
            .await
            .is_err());
        let after = store
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before, after);
        assert!(crate::store::ledger_collection_progress::retention_required(&after).unwrap());
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn checkpoint_mongo_retention_conflicts_with_purge_snapshot() {
    let (client, name) = database("checkpoint_purge").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = retired_sandbox("sbx_checkpoint_purge");
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut purge = client.start_session().await.unwrap();
    start_transaction(&mut purge).await.unwrap();
    store
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut purge)
        .await
        .unwrap()
        .unwrap();
    store
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint())
        .await
        .unwrap();
    assert!(store
        .sandboxes
        .delete_one(doc! {"_id":parent.id.as_str(),"revision":parent.revision as i64})
        .session(&mut purge)
        .await
        .is_err());
    let _ = purge.abort_transaction().await;
    assert!(store
        .purge_sandbox(&parent.id, parent.revision, NOW)
        .await
        .is_err());
    assert_eq!(
        store
            .ledger_collection_checkpoint(&parent.id)
            .await
            .unwrap(),
        Some(checkpoint())
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with test failpoints"]
async fn checkpoint_mongo_lost_write_ack_reconciles_exact_original_reference() {
    let (admin, name) = database("checkpoint_unknown_write").await;
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app = format!("checkpoint-unknown-{}", uuid::Uuid::new_v4());
    options.app_name = Some(app.clone());
    options.retry_writes = Some(false);
    let client = mongodb::Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_checkpoint_unknown_write");
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .retain_ledger_collection_checkpoint(&parent.id, parent.revision, checkpoint())
        .await
        .unwrap();
    let state = store
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            1,
            M::PinPage {
                fence: 1,
                now: 11,
                page: page(),
            },
        )
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":1},"data":{
                "failCommands":["update"],"appName":app,
                "writeConcernError":{"code":64,"errmsg":"fixture acknowledgement unavailable"}
            }
        })
        .await
        .unwrap();
    let result = store
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::RecordReference {
                fence: 1,
                now: 11,
                reference: reference(),
            },
        )
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let current = reopened
        .ledger_collection_checkpoint(&parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.accepted_artifact, Some(reference()));
    assert_eq!(current.progress.revision, state.progress.revision + 1);
    assert!(reopened
        .update_ledger_collection_checkpoint(
            &parent.id,
            parent.revision,
            state.progress.revision,
            M::RecordReference {
                fence: 1,
                now: 11,
                reference: reference()
            }
        )
        .await
        .is_err());
    assert_eq!(
        reopened
            .update_ledger_collection_checkpoint(
                &parent.id,
                parent.revision,
                current.progress.revision,
                M::RecordReference {
                    fence: 1,
                    now: 11,
                    reference: reference()
                }
            )
            .await
            .unwrap(),
        current
    );
    admin.database(&name).drop().await.unwrap();
}

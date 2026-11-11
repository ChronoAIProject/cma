use super::*;
use layer2_core::sandbox::store::ledger_collection::{
    LedgerCollectionPageProgress, LedgerCollectionPending, LedgerCollectionPhase,
    LedgerCollectionStore, LedgerCollectorMutation, LedgerCollectorProgressV1,
};

#[path = "ledger_collection_checkpoint_tests.rs"]
mod checkpoint;

fn progress() -> LedgerCollectorProgressV1 {
    LedgerCollectorProgressV1::new("a".repeat(64), 1, 7, "b".repeat(64), 10, 20).unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn collector_two_workers_use_parent_and_progress_cas_and_preserve_restart_identity() {
    let (client, name) = database("collector_cas").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_collector_cas");
    first.create_sandbox(parent.clone()).await.unwrap();
    first
        .retain_ledger_collection(&parent.id, parent.revision, progress())
        .await
        .unwrap();
    first
        .retain_ledger_collection(&parent.id, parent.revision, progress())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        first.update_ledger_collection(
            &parent.id,
            parent.revision,
            1,
            LedgerCollectorMutation::Claim {
                holder_sha256: "c".repeat(64),
                now: 20,
                expires_at: 30
            }
        ),
        second.update_ledger_collection(
            &parent.id,
            parent.revision,
            1,
            LedgerCollectorMutation::Claim {
                holder_sha256: "d".repeat(64),
                now: 20,
                expires_at: 30
            }
        ),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let current = a.or(b).unwrap();
    assert_eq!(current.revision, 2);
    assert_eq!(current.lease.fence, 2);
    let page = LedgerCollectionPageProgress {
        page_sha256: "e".repeat(64),
        active_revision: 7,
        cursor: None,
        next_cursor: Some("page-64".into()),
        complete: false,
        remaining_groups: vec!["1".repeat(64)],
    };
    let pinned = first
        .update_ledger_collection(
            &parent.id,
            parent.revision,
            2,
            LedgerCollectorMutation::PinPage {
                fence: 2,
                now: 21,
                page,
            },
        )
        .await
        .unwrap();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let selected = first
        .update_ledger_collection(
            &parent.id,
            parent.revision,
            pinned.revision,
            LedgerCollectorMutation::Select {
                fence: 2,
                now: 21,
                pending: LedgerCollectionPending {
                    operation_id: operation_id.clone(),
                    group_key: "1".repeat(64),
                    request_sha256: "2".repeat(64),
                    observation_sha256: "3".repeat(64),
                    source_sha256: "4".repeat(64),
                    phase: LedgerCollectionPhase::Selected,
                    receipt_sha256: None,
                },
            },
        )
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .ledger_collection_progress(&parent.id)
            .await
            .unwrap()
            .unwrap(),
        selected
    );
    let takeover = reopened
        .update_ledger_collection(
            &parent.id,
            parent.revision,
            selected.revision,
            LedgerCollectorMutation::Claim {
                holder_sha256: "f".repeat(64),
                now: 30,
                expires_at: 40,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        takeover.pending.as_ref().unwrap().operation_id,
        operation_id
    );
    assert_eq!(takeover.page, selected.page);
    assert!(reopened
        .update_ledger_collection(
            &parent.id,
            parent.revision,
            takeover.revision,
            LedgerCollectorMutation::Advance {
                fence: 3,
                now: 31,
                active_revision: 7,
                page_sha256: "e".repeat(64)
            }
        )
        .await
        .is_err());
    assert!(reopened
        .update_ledger_collection(
            &parent.id,
            parent.revision + 1,
            takeover.revision,
            LedgerCollectorMutation::Rollover {
                fence: 3,
                now: 31,
                active_revision: 8
            }
        )
        .await
        .is_err());
    assert_eq!(
        first.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parent
    );
    let mut changed = parent.clone();
    changed.revision += 1;
    changed.updated_at = MINUTE.into();
    first
        .save_sandbox(parent.revision, changed.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .ledger_collection_progress(&parent.id)
            .await
            .unwrap(),
        Some(takeover.clone()),
        "ordinary parent saves must preserve the separate retention/progress marker"
    );
    assert!(matches!(
        reopened
            .update_ledger_collection(
                &parent.id,
                parent.revision,
                takeover.revision,
                LedgerCollectorMutation::Claim {
                    holder_sha256: "e".repeat(64),
                    now: 40,
                    expires_at: 50,
                },
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(first.get_sandbox(&parent.id).await.unwrap(), Some(changed));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn collector_retention_and_unreleased_home_block_actual_purge() {
    let (client, name) = database("collector_purge_hold").await;
    for case in 0..4 {
        let db = format!("{name}_{case}");
        let parent = if case == 0 {
            hierarchy().0
        } else {
            retired_sandbox("sbx_collector_retained")
        };
        parent.validate().unwrap();
        if case == 0 {
            assert!(!parent.lifecycle.home.as_ref().unwrap().released);
            let mut impossible = parent.clone();
            impossible.status = SandboxStatus::Retired;
            assert!(impossible.validate().is_err());
        } else {
            assert_eq!(parent.status, SandboxStatus::Retired);
        }
        let store = MongoSandboxStore::open(&client, &db).await.unwrap();
        store.create_sandbox(parent.clone()).await.unwrap();
        if case == 1 {
            store
                .retain_ledger_collection(&parent.id, parent.revision, progress())
                .await
                .unwrap();
        } else if case >= 2 {
            let marker = if case == 2 {
                Bson::Null
            } else {
                Bson::Document(doc! {"format":"future"})
            };
            store
                .sandboxes
                .update_one(
                    doc! {"_id":parent.id.as_str()},
                    doc! {"$set":{"ledger_collection_v1":marker}},
                )
                .await
                .unwrap();
        }
        let persisted = store
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        if case == 0 {
            assert!(!persisted.contains_key("ledger_collection_v1"));
        }
        assert!(crate::store::ledger_collection_progress::retention_required(&persisted).unwrap());
        assert!(matches!(
            store.purge_sandbox(&parent.id, parent.revision, NOW).await,
            Err(SandboxStoreError::Conflict)
        ));
        assert!(store.get_sandbox(&parent.id).await.unwrap().is_some());
        client.database(&db).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn collector_retention_conflicts_with_a_purge_snapshot_and_cannot_resurrect_a_purged_parent()
{
    let (client, name) = database("collector_purge_race").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = retired_sandbox("sbx_collector_purge_race");
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut purge = client.start_session().await.unwrap();
    start_transaction(&mut purge).await.unwrap();
    let before = store
        .sandboxes
        .find_one(doc! {"_id":parent.id.as_str()})
        .session(&mut purge)
        .await
        .unwrap()
        .unwrap();
    assert!(!crate::store::ledger_collection_progress::retention_required(&before).unwrap());
    store
        .retain_ledger_collection(&parent.id, parent.revision, progress())
        .await
        .unwrap();
    // Same final parent-delete CAS as purge_in_transaction. A separate marker
    // collection would permit write skew here; the parent-row marker conflicts.
    let deleted = store
        .sandboxes
        .delete_one(doc! {"_id":parent.id.as_str(),"revision":parent.revision as i64})
        .session(&mut purge)
        .await;
    assert!(deleted.is_err());
    let _ = purge.abort_transaction().await;
    assert_eq!(
        store.ledger_collection_progress(&parent.id).await.unwrap(),
        Some(progress())
    );

    let other = retired_sandbox("sbx_collector_purge_winner");
    store.create_sandbox(other.clone()).await.unwrap();
    store
        .purge_sandbox(&other.id, other.revision, NOW)
        .await
        .unwrap();
    assert!(store
        .retain_ledger_collection(&other.id, other.revision, progress())
        .await
        .is_err());
    assert!(store.get_sandbox(&other.id).await.unwrap().is_none());
    client.database(&name).drop().await.unwrap();
}

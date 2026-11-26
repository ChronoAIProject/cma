use super::*;
use crate::store::agent_log_segments as segments;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
#[path = "mutation_contention/artifacts.rs"]
mod artifacts;
#[path = "mutation_contention/segmented.rs"]
mod segmented;
#[path = "mutation_contention/support.rs"]
mod support;
use support::Fixture;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn share_fence_accepts_valid_segmented_agent_and_preserves_its_log_envelope() {
    let f = Fixture::new(false).await;
    segments::schema::migrate(&f.admin.database(&f.name))
        .await
        .unwrap();
    let agent = f.before.lineage.agent.clone().unwrap();
    let logs = f.store.agent_log(&agent.id, 0, 20).await.unwrap();
    f.store
        .agents
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    f.store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    segments::mongo_tests::fixtures::install(&f.store, agent.clone(), logs.clone(), 0, 2).await;
    let original = f
        .store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let snapshot = f.store.resource_access_snapshot(&f.target).await.unwrap();
    assert_eq!(
        snapshot, f.before,
        "the version-two envelope is valid before fencing"
    );
    let request = mutation(&snapshot, "segmented-share", ShareMode::View);
    let receipt = f.store.mutate_share(request.clone()).await.unwrap();
    assert_eq!(f.store.mutate_share(request).await.unwrap(), receipt);
    let mut expected = original;
    expected.insert("resource_share_guard", 1_i64);
    assert_eq!(
        f.store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        expected
    );
    assert_eq!(f.store.agent_log(&agent.id, 0, 20).await.unwrap(), logs);
    assert_eq!(
        f.store
            .resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let current = f.store.resource_access_snapshot(&f.target).await.unwrap();
    let next = mutation(&current, "invalid-envelope", ShareMode::View);
    for fields in [
        doc! {"document_schema_version":3_i64},
        doc! {"document_schema_version":2_i64,"agent_log_incarnation":"f".repeat(64)},
    ] {
        f.store
            .agents
            .update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":fields})
            .await
            .unwrap();
        f.trace.commands.lock().unwrap().clear();
        assert_eq!(
            f.store.mutate_share(next.clone()).await,
            Err(ResourceAccessStoreError::Corrupt)
        );
        assert_eq!(
            f.trace.writes(),
            0,
            "unsupported or torn envelopes fail before any fence"
        );
        assert_eq!(
            f.store
                .resource_share_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    f.destroy().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn share_contention_reloads_lineage_and_keeps_original_identity_and_timestamp() {
    let f = Fixture::new(false).await;
    let request = mutation(&f.before, "metadata-race", ShareMode::View);
    f.trace.pause.store(true, Ordering::SeqCst);
    let mutator = f.store.clone();
    let admitted = request.clone();
    let pending = tokio::spawn(async move { mutator.mutate_share(admitted).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.trace.entered.notified(),
    )
    .await
    .unwrap();
    let agent = f.before.lineage.agent.as_ref().unwrap();
    let result = f
        .admin
        .database(&f.name)
        .collection::<Document>("agents")
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$inc":{"revision":1_i64,"record.revision":1_i64},
            "$set":{"updated_at":MINUTE,"record.updated_at":MINUTE}},
        )
        .await;
    f.trace.release();
    assert_eq!(result.unwrap().matched_count, 1);
    let receipt = pending.await.unwrap().unwrap();
    assert_eq!(
        f.trace.snapshots(),
        2,
        "retry must read fresh full ancestry"
    );
    assert_eq!(receipt.at, request.at);
    assert_eq!(receipt.share.id, request.proposed_id);
    assert_eq!(receipt.operation_id, request.operation_id());
    assert_eq!(receipt.command_fingerprint, request.fingerprint());
    assert_eq!(f.store.mutate_share(request).await.unwrap(), receipt);
    let after = f.store.resource_access_snapshot(&f.target).await.unwrap();
    let current = after.lineage.agent.unwrap();
    assert_eq!(current.revision, agent.revision + 1);
    assert_eq!(current.updated_at, MINUTE);
    assert_eq!(current.conversation, agent.conversation);
    assert_eq!(
        f.store
            .resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.destroy().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn share_contention_does_not_refresh_the_original_policy_authority() {
    let f = Fixture::new(false).await;
    let request = mutation(&f.before, "stale-policy", ShareMode::View);
    f.trace.pause.store(true, Ordering::SeqCst);
    let mutator = f.store.clone();
    let admitted = request.clone();
    let pending = tokio::spawn(async move { mutator.mutate_share(admitted).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.trace.entered.notified(),
    )
    .await
    .unwrap();
    let writer = MongoSandboxStore::open(&f.admin, &f.name).await.unwrap();
    let changed = writer
        .mutate_share(mutation(&f.before, "other-policy", ShareMode::View))
        .await;
    f.trace.release();
    let committed = changed.unwrap();
    assert_eq!(
        pending.await.unwrap(),
        Err(ResourceAccessStoreError::Conflict)
    );
    assert_eq!(f.trace.snapshots(), 2);
    assert!(f
        .store
        .share_mutation_receipt(&request.operation_id())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.store
            .resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.store.resource_share(&committed.share.id).await.unwrap(),
        Some(committed.share)
    );
    f.destroy().await;
}

async fn share_known_abort_converges_but_exhaustion_publishes_nothing_case(failures: usize) {
    let f = Fixture::new(true).await;
    let request = mutation(&f.before, "known-abort", ShareMode::View);
    f.fail_writes(failures as i32, &["TransientTransactionError"])
        .await;
    let result = f.store.mutate_share(request.clone()).await;
    f.clear_failpoint().await;
    assert_eq!(
        f.trace.snapshots(),
        (failures + 1).min(TRANSACTION_ATTEMPTS)
    );
    if failures == TRANSACTION_ATTEMPTS {
        assert_eq!(result, Err(ResourceAccessStoreError::Conflict));
        f.assert_no_publication().await;
    } else {
        let receipt = result.unwrap();
        assert_eq!(receipt.at, request.at);
        assert_eq!(receipt.share.id, request.proposed_id);
        assert_eq!(f.store.mutate_share(request).await.unwrap(), receipt);
        assert_eq!(
            f.store
                .resource_share_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn share_unknown_write_result_overrides_transient_abort_and_never_replays() {
    let f = Fixture::new(true).await;
    let request = mutation(&f.before, "unknown-write", ShareMode::View);
    f.fail_writes(
        1,
        &[
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
    let result = f.store.mutate_share(request).await;
    f.clear_failpoint().await;
    assert_eq!(result, Err(ResourceAccessStoreError::Unavailable));
    assert_eq!(
        f.trace.writes(),
        1,
        "an unknown result must not start another mutation"
    );
    assert_eq!(f.trace.snapshots(), 1);
    f.assert_no_publication().await;
    f.destroy().await;
}

#[path = "mutation_contention_cases.rs"]
mod cases;

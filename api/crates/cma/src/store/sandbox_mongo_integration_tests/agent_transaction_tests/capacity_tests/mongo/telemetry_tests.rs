//! Assert attribution at production caller boundaries, including rejected publications.
use super::*;
use crate::store::agent_log_telemetry::{isolated_test, snapshot, ScanCaller};
use layer2_core::sandbox::store::plan_agent_cleanup_capacity_retry;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn complete_scan_callers_distinguish_worker_settlement_proof_and_public_prefix() {
    let (client, name) = database("scan_callers").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "scan_callers").await;
    let agent = fill_normal_capacity(&store, agent, false).await;
    let store = prepare_capacity_fixture(store, &agent).await;
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: MINUTE.into(),
        })
        .await
        .unwrap();
    isolated_test(async {
        let fenced = claim_interaction_fence(&store, &agent.id).await;
        store
            .commit_agent_work(advance(&fenced, AgentWorkStep::CloseProcess, MINUTE))
            .await
            .unwrap();
        // Use the performed step, not the proposed next step.
        assert_eq!(
            snapshot(
                ScanCaller::WorkerPublication(AgentWorkStep::FenceInteraction),
                true
            )[0],
            1
        );
        assert_eq!(
            snapshot(
                ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess),
                true
            )[0],
            0
        );
        let current = claim(&store, &agent.id, MINUTE).await;
        let tail = store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap();
        let retry = plan_agent_cleanup_capacity_retry(
            &current,
            tail.as_ref(),
            "worker",
            current.work.lease.as_ref().unwrap().fence,
            MINUTE,
            TWO_MINUTES,
        )
        .unwrap();
        let mut ordinary = retry.clone();
        ordinary.agent_logs = vec![AgentLogKind::WorkStepFailed {
            step: current.work.step,
            failure: TypedFailure {
                class: "runtime_unavailable".into(),
                detail: "retry".into(),
            },
        }];
        assert_eq!(
            store.commit_agent_work(ordinary).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert_eq!(
            &snapshot(
                ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess),
                true
            )[..2],
            &[1, 10_001]
        );
        store.commit_agent_work(retry.clone()).await.unwrap();
        assert_eq!(
            &snapshot(
                ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
                true
            )[..2],
            &[1, 10_001]
        );
        let receipt = crate::store::agent_worker_transaction::PublicationReceipt {
            segmented: None,
            agent: retry.agent.clone(),
            worktree: None,
            logs: vec![],
            event: None,
            sign_in_sandbox: None,
        };
        assert!(
            crate::store::agent_worker_reconcile::reconcile_publication(&store, &receipt)
                .await
                .unwrap()
        );
        assert_eq!(
            &snapshot(ScanCaller::UnknownCommitProof, true)[..2],
            &[1, 10_002]
        );
        assert_eq!(store.agent_log_prefix(&agent.id, 2).await.unwrap().len(), 2);
        assert_eq!(
            &snapshot(ScanCaller::PublicWindowPrefix, true)[..2],
            &[0, 0],
            "prepared bounded windows are not complete scans"
        );
        // Disable the local cohort while retaining metadata to exercise the
        // complete public fallback and its failure attribution separately.
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(store.agent_log_prefix(&agent.id, 2).await.unwrap().len(), 2);
        assert_eq!(
            &snapshot(ScanCaller::PublicWindowPrefix, true)[..2],
            &[1, 10_002]
        );
        store
            .agent_log
            .update_one(
                doc! {"agent_id": agent.id.as_str(), "seq": 1_i64},
                doc! {"$set": {"document_schema_version": 999_i64}},
            )
            .await
            .unwrap();
        assert!(store
            .agent_log_range(&agent.id, 10_002, 10_002, 10)
            .await
            .is_err());
        let failed = snapshot(ScanCaller::PublicWindowPrefix, false);
        assert_eq!(&failed[..2], &[1, 1]);
        assert!(failed[2] > 0);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

// The retained capacity helper deliberately starts at ConversationOpened.
// Establish a complete legacy history before preparation, preserving its head
// and capacity. This test must exercise a valid, selectable checkpoint.
async fn prepare_capacity_fixture(store: MongoSandboxStore, agent: &Agent) -> MongoSandboxStore {
    for (seq, kind) in [
        (
            1,
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.creation_worktree_id().clone(),
            },
        ),
        (
            2,
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
        ),
    ] {
        store
            .agent_log
            .replace_one(
                doc! { "agent_id": agent.id.as_str(), "seq": seq as i64 },
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$unset": { crate::store::agent_log_integrity::INTEGRITY_FIELD: "" } },
        )
        .await
        .unwrap();
    store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn prepared_capacity_zero_entry_retry_still_refuses_middle_gap() {
    let (client, name) = database("prepared_capacity_gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "prepared_capacity_gap").await;
    let agent = fill_normal_capacity(&store, agent, false).await;
    let store = prepare_capacity_fixture(store, &agent).await;
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: MINUTE.into(),
        })
        .await
        .unwrap();
    let fenced = claim_interaction_fence(&store, &agent.id).await;
    store
        .commit_agent_work(advance(&fenced, AgentWorkStep::CloseProcess, MINUTE))
        .await
        .unwrap();
    let current = claim(&store, &agent.id, MINUTE).await;
    let tail = store
        .agent_log_entry(&current.id, current.conversation.last_seq)
        .await
        .unwrap();
    let first_retry = plan_agent_cleanup_capacity_retry(
        &current,
        tail.as_ref(),
        "worker",
        current.work.lease.as_ref().unwrap().fence,
        MINUTE,
        TWO_MINUTES,
    )
    .unwrap();
    store.commit_agent_work(first_retry).await.unwrap();
    let current = claim(&store, &agent.id, TWO_MINUTES).await;
    let tail = store
        .agent_log_entry(&current.id, current.conversation.last_seq)
        .await
        .unwrap();
    let retry = plan_agent_cleanup_capacity_retry(
        &current,
        tail.as_ref(),
        "worker",
        current.work.lease.as_ref().unwrap().fence,
        TWO_MINUTES,
        "2026-09-05T00:03:00Z",
    )
    .unwrap();
    assert!(retry.agent_logs.is_empty());
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(
        crate::store::agent_log_integrity::parse_integrity(&before)
            .unwrap()
            .unwrap()
            .semantic
            .valid
    );
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 500_i64 })
        .await
        .unwrap();
    isolated_test(async {
        assert!(store.commit_agent_work(retry).await.is_err());
        assert_eq!(
            snapshot(
                ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
                false
            )[0],
            1
        );
    })
    .await;
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(current));
    let after = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(after.contains_key(crate::store::agent_log_integrity::QUARANTINE_FIELD));
    assert_eq!(
        before.get(crate::store::agent_log_integrity::INTEGRITY_FIELD),
        after.get(crate::store::agent_log_integrity::INTEGRITY_FIELD)
    );
    client.database(&name).drop().await.unwrap();
}

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
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: MINUTE.into(),
        })
        .await
        .unwrap();
    isolated_test(async {
        let fenced = claim(&store, &agent.id, MINUTE).await;
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

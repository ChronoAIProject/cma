use super::*;
use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryClass, WorkRetryFailure};
use std::time::{Duration, SystemTime};

fn stamp(seconds: u64) -> String {
    layer2_core::rfc3339_at(
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(layer2_core::seconds_from_rfc3339(NOW).unwrap() + seconds),
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retry_policy_mongo_agent_due_time_survives_reopen_and_lease_takeover() {
    let (client, name) = database("retry_policy_agent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, mut current) = claimed_agent_work_fixture(&store, "retry_policy").await;
    // The existing fixture establishes the lineage and initial log evidence.
    current.requested_stop_reason = Some(layer2_core::sandbox::AgentStopReason::Owner);
    current.work.step = AgentWorkStep::CloseProcess;
    replace_agent_fixture(&store, &current).await;
    let mut seconds = 0;
    for attempt in 0..14 {
        let fence = current.work.lease.as_ref().unwrap().fence;
        let decision = work_retry_policy(
            WorkRetryClass::Maintenance,
            WorkRetryFailure::Transient,
            current.work.attempts,
            Duration::from_secs(30),
        );
        let due = seconds + decision.delay.as_secs();
        let mut next = current.clone();
        next.work.retry("worker", fence, stamp(due)).unwrap();
        next.revision += 1;
        next.updated_at = stamp(seconds);
        next.conversation.last_seq += 1;
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: current.revision,
                holder: "worker".into(),
                fence,
                agent: next.clone(),
                worktree: None,
                appended_at: stamp(seconds),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepFailed {
                    step: current.work.step,
                    failure: TypedFailure {
                        class: "agent_runtime_unavailable".into(),
                        detail: "fixture".into(),
                    },
                }],
            })
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(reopened.get_agent(&current.id).await.unwrap(), Some(next));
        assert!(reopened
            .due_agent_work(&stamp(due - 1), 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened.due_agent_work(&stamp(due), 10).await.unwrap(),
            vec![current.id.clone()]
        );
        current = reopened
            .claim_agent_work(&current.id, "worker", stamp(due), stamp(due + 60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.work.attempts, attempt + 1);
        if attempt == 12 {
            let stale = current.clone();
            current = reopened
                .claim_agent_work(&current.id, "worker", stamp(due + 60), stamp(due + 120))
                .await
                .unwrap()
                .unwrap();
            assert!(
                current.work.lease.as_ref().unwrap().fence
                    > stale.work.lease.as_ref().unwrap().fence
            );
            assert_eq!(current.work.attempts, stale.work.attempts);
            seconds = due + 60;
        } else {
            seconds = due;
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retry_policy_mongo_sandbox_probe_is_pending_and_discovered_only_when_due() {
    let (client, name) = database("retry_policy_sandbox").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut initial = sandbox("sbx_retry_policy");
    initial.status = SandboxStatus::Retiring;
    initial
        .work
        .schedule(SandboxWorkStep::StopAgents, NOW.into());
    store.create_sandbox(initial.clone()).await.unwrap();
    let mut seconds = 0;
    for attempt in 0..5 {
        let current = store
            .claim_sandbox_work(&initial.id, "worker", stamp(seconds), work_lease_deadline())
            .await
            .unwrap()
            .unwrap();
        let decision = work_retry_policy(
            WorkRetryClass::Maintenance,
            WorkRetryFailure::ExternalAction,
            current.work.attempts,
            Duration::from_secs(30),
        );
        let due = seconds + decision.delay.as_secs();
        let mut next = current.clone();
        next.work
            .retry(
                "worker",
                current.work.lease.as_ref().unwrap().fence,
                stamp(due),
            )
            .unwrap();
        next.revision += 1;
        next.updated_at = stamp(seconds);
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: current.revision,
                sandbox: Box::new(next.clone()),
                event_at: stamp(seconds),
                event: SandboxEventKind::WorkStepFailed {
                    step: current.work.step,
                    failure: TypedFailure {
                        class: "terminal_security".into(),
                        detail: "fixture".into(),
                    },
                },
            })
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(reopened.get_sandbox(&initial.id).await.unwrap(), Some(next));
        assert!(reopened
            .due_sandbox_work(&stamp(due - 1), 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened.due_sandbox_work(&stamp(due), 10).await.unwrap(),
            vec![initial.id.clone()]
        );
        assert_eq!(decision.parked, attempt >= 3);
        seconds = due;
    }
    client.database(&name).drop().await.unwrap();
}

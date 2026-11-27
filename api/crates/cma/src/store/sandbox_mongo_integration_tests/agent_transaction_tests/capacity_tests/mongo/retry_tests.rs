use super::*;
use layer2_core::sandbox::store::plan_agent_cleanup_capacity_retry;
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use std::time::{Duration, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cleanup_capacity_retries_preserve_reserve_until_runtime_recovers() {
    let (client, name) = database("capacity_cleanup_retry").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "capacity_retry").await;
    let agent = fill_normal_capacity(&store, agent, false).await;
    let original_head = agent.conversation.last_seq;
    let original_history = fingerprint(&store, &agent.id, original_head).await;
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
    let start = seconds_from_rfc3339(MINUTE).unwrap();
    // More retries than the entire 64-entry stop reserve can otherwise hold.
    for attempt in 0..70 {
        let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(start + attempt));
        let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(start + attempt + 1));
        let current = claim(&store, &agent.id, &at).await;
        let last = store
            .agent_log(&agent.id, current.conversation.last_seq - 1, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let fence = current.work.lease.as_ref().unwrap().fence;
        if attempt == 0 {
            let mut ordinary = plan_agent_cleanup_capacity_retry(
                &current,
                Some(&last),
                "worker",
                fence,
                &at,
                &due,
            )
            .unwrap();
            ordinary.agent_logs = vec![AgentLogKind::WorkStepFailed {
                step: current.work.step,
                failure: TypedFailure {
                    class: "runtime_unavailable".into(),
                    detail: "retry".into(),
                },
            }];
            assert_eq!(
                store.commit_agent_work(ordinary.clone()).await,
                Err(SandboxStoreError::AgentLogCapacityExceeded)
            );
            ordinary.agent_logs.clear();
            ordinary.agent.conversation.last_seq -= 1;
            assert_eq!(
                store.commit_agent_work(ordinary).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap(),
                Some(current.clone())
            );
        } else if attempt == 1 {
            let duplicate =
                plan_agent_cleanup_capacity_retry(&current, None, "worker", fence, &at, &due)
                    .unwrap();
            assert_eq!(
                store.commit_agent_work(duplicate).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap(),
                Some(current.clone())
            );
        }
        let retry =
            plan_agent_cleanup_capacity_retry(&current, Some(&last), "worker", fence, &at, &due)
                .unwrap();
        store.commit_agent_work(retry.clone()).await.unwrap();
        if attempt == 1 {
            let mut receipt = crate::store::agent_worker_transaction::PublicationReceipt {
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
            receipt.agent.work.attempts += 1;
            assert!(
                !crate::store::agent_worker_reconcile::reconcile_publication(&store, &receipt)
                    .await
                    .unwrap()
            );
        }
        let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(saved, retry.agent);
        assert_eq!(saved.conversation.last_seq, original_head + 2);
        assert_eq!(saved.work.attempts, attempt as u32 + 1);
        assert!(saved.work.lease.is_none());
        assert!(!saved.work.is_due(&at));
        assert!(saved.work.is_due(&due));
        assert_eq!(saved.work.receipts, current.work.receipts);
        assert_eq!(saved.work.cleanup_debt, current.work.cleanup_debt);
    }
    let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(start + 70));
    let current = claim(&store, &agent.id, &at).await;
    let mut closed = advance(&current, AgentWorkStep::PublishStopped, &at);
    add_receipt(&mut closed, AgentWorkStep::CloseProcess);
    store.commit_agent_work(closed).await.unwrap();
    let publishing = claim(&store, &agent.id, &at).await;
    let mut stopped = advance(&publishing, AgentWorkStep::None, &at);
    stopped.agent.revision -= 1;
    stopped
        .agent
        .transition(
            AgentStatus::Stopped,
            at.clone(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.agent_logs = vec![AgentLogKind::Stopped {
        reason: "owner".into(),
    }];
    stopped.sandbox_event = Some(SandboxEventKind::AgentStatusChanged {
        agent_id: agent.id.clone(),
        from: publishing.status,
        to: AgentStatus::Stopped,
    });
    store.commit_agent_work(stopped.clone()).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_agent(&agent.id).await.unwrap(),
        Some(stopped.agent)
    );
    assert_eq!(
        fingerprint(&reopened, &agent.id, original_head).await,
        original_history
    );
    let tail = reopened
        .agent_log(&agent.id, original_head, 200)
        .await
        .unwrap();
    assert_eq!(tail.len(), 4);
    assert!(matches!(&tail[1].kind, AgentLogKind::WorkStepFailed { .. }));
    assert_eq!(
        tail[3].kind,
        AgentLogKind::Stopped {
            reason: "owner".into()
        }
    );
    client.database(&name).drop().await.unwrap();
}

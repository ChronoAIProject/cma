use super::*;
use layer2_core::sandbox::store::AgentStopRequest;

fn worker(h: &Harness, enabled: bool) -> Arc<AgentWorker> {
    AgentWorker::new(
        AgentWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new("cleanup-scheduled", DEPLOYMENT_ID)
            .with_cleanup_continuation(enabled),
    )
}

#[tokio::test]
async fn cleanup_continuation_preserves_fresh_claims_and_exact_effect_order() {
    for enabled in [false, true] {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = h.admit("scheduled-cleanup", vec![]).await;
        let ready = h.drive_ready(&admitted.id).await;
        let start = SystemTime::now().max(h.now);
        let before = h.backend.requests().await.len();
        h.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&ready),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(start),
            })
            .await
            .unwrap();
        let scheduled = h.stored(&ready.id).await;
        let runner = worker(&h, enabled);
        for _ in 0..if enabled { 1 } else { 3 } {
            assert!(runner.run_scheduled(&ready.id, start).await.unwrap());
        }
        let stopped = h.stored(&ready.id).await;
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert_eq!(stopped.work.step, AgentWorkStep::None);
        assert_eq!(stopped.work.next_fence, scheduled.work.next_fence + 3);
        h.store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&stopped),
                requested_at: rfc3339_at(start),
            })
            .await
            .unwrap();
        let deleting = h.stored(&ready.id).await;
        for _ in 0..if enabled { 1 } else { 4 } {
            assert!(runner.run_scheduled(&ready.id, start).await.unwrap());
        }
        let deleted = h.stored(&ready.id).await;
        assert_eq!(deleted.status, AgentStatus::Deleted);
        assert_eq!(deleted.work.step, AgentWorkStep::None);
        assert_eq!(deleted.work.next_fence, deleting.work.next_fence + 4);
        assert_eq!(
            h.store
                .get_worktree(&ready.worktree_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorktreeStatus::Removed
        );
        assert!(!runner.run_scheduled(&ready.id, start).await.unwrap());
        let requests = h.backend.requests().await;
        assert_eq!(
            requests[before..]
                .iter()
                .map(|(_, request)| request.operation())
                .collect::<Vec<_>>(),
            [
                WireOperation::CloseAgent,
                WireOperation::RemoveAgentHome,
                WireOperation::RemoveWorktree
            ]
        );
        let fences = requests[before..]
            .iter()
            .map(|(_, request)| ledger_hold_tests::request_identity(request).0)
            .collect::<Vec<_>>();
        assert!(fences.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

#[tokio::test]
async fn cleanup_continuation_yields_on_ledger_hold_and_preserves_retry_identity() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let initial = ledger_hold_tests::prepare(&h, AgentWorkStep::RemoveHome).await;
    let start = ledger_hold_tests::due(&initial);
    let before = h.backend.requests().await.len();
    ledger_hold_tests::refuse(&h, WireOperation::RemoveAgentHome);
    let runner = worker(&h, true);
    assert!(runner.run_scheduled(&initial.id, start).await.unwrap());
    let pending = h.stored(&initial.id).await;
    ledger_hold_tests::preserved(&initial, &pending);
    assert_eq!(pending.work.attempts, 1);
    assert_eq!(h.backend.requests().await.len(), before + 1);
    assert!(!runner.run_scheduled(&initial.id, start).await.unwrap());
    let retry = ledger_hold_tests::due(&pending);
    assert!(runner.run_scheduled(&initial.id, retry).await.unwrap());
    assert_eq!(h.stored(&initial.id).await.status, AgentStatus::Deleted);
    let requests = h.backend.requests().await;
    let first = ledger_hold_tests::request_identity(&requests[before].1);
    let second = ledger_hold_tests::request_identity(&requests[before + 1].1);
    assert!(second.0 > first.0);
    assert_eq!((first.1, first.2), (second.1, second.2));
}

#[tokio::test]
async fn cleanup_continuation_cannot_take_another_live_claim() {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let initial = ledger_hold_tests::prepare(&h, AgentWorkStep::RemoveHome).await;
    let start = ledger_hold_tests::due(&initial);
    h.store
        .claim_agent_work(
            &initial.id,
            "other-holder",
            rfc3339_at(start),
            rfc3339_at(start + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let before = h.backend.requests().await.len();
    assert!(!worker(&h, true)
        .run_scheduled(&initial.id, start)
        .await
        .unwrap());
    assert_eq!(h.backend.requests().await.len(), before);
    assert_eq!(
        h.stored(&initial.id).await.work.step,
        AgentWorkStep::RemoveHome
    );
}

mod durable_control_tests;

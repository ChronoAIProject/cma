use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn native_failure_refuses_replacement_before_first_publication() {
    use layer3_adapter::wire::RequestFrame;

    for response_lost in [false, true] {
        let fixture = Fixture::new().await;
        let h = &fixture.bootstrap.harness;
        let waiting = fixture.bootstrap.admit(None).await;
        fixture.bootstrap.ready().await;
        assert!(h
            .worker
            .run_one(&waiting.id, SystemTime::now())
            .await
            .unwrap());
        let (claimed, now) = fixture.claim(&waiting.id).await;
        assert_eq!(claimed.work.step, AgentWorkStep::AddWorktree);
        let original_runtime = h
            .store
            .get_sandbox(&claimed.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .runtime
            .unwrap();
        let previous_requests = h.backend.requests().await.len();
        if response_lost {
            h.backend.lose_next_response(WireOperation::AddWorktree);
        } else {
            h.backend.fail_next(
                WireOperation::AddWorktree,
                "runtime_mutation_outcome_unknown",
                "synthetic native worktree refusal",
            );
        }
        *fixture.trace.replace_before_failure.lock().unwrap() =
            Some((claimed.id.clone(), h.backend.clone()));
        *fixture.trace.pending_fault.lock().unwrap() =
            Some((claimed.id.clone(), Fault::KnownAbort));

        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), now)
            .await;
        fixture.disable().await;
        let current = h.stored(&claimed.id).await;
        let prepared = fixture.trace.prepared.lock().unwrap().take().unwrap();
        let replacement = h
            .store
            .get_sandbox(&claimed.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .runtime
            .unwrap();
        let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
        let requests = h.backend.requests().await;
        let sends: Vec<_> = requests[previous_requests..]
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::AddWorktree(_)))
            .collect();
        fixture.cleanup().await;

        assert!(fixture
            .trace
            .replace_before_failure
            .lock()
            .unwrap()
            .is_none());
        assert_ne!(replacement, original_runtime);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].0 .0, original_runtime.runtime_id);
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_eq!(
            current, prepared,
            "the original native intent must remain pending"
        );
        assert!(!logs.iter().any(|item| matches!(
            item.kind,
            AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::AddWorktree,
                ..
            }
        )));
        assert!(fixture.trace.bodies.load(Ordering::SeqCst) <= 1);
    }
}

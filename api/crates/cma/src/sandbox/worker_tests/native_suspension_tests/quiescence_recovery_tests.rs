use super::*;
use crate::sandbox::worker_tests::native_suspension_tests::backend::RetirementProbePause;
use layer2_core::sandbox::SandboxStoreError;

#[tokio::test]
async fn quiescence_takeover_fences_stale_park_and_replays_lost_suspend_receipt() {
    let (fixture, admitted, workspace, mut now) = admitted_generation(Some(2)).await;
    let original = fixture.backend.snapshot().await;
    fixture.backend.nonexecuting.store(false, Ordering::SeqCst);
    let worker =
        fixture
            .harness
            .worker_with_runtime_image("first-manager", fixture.adapter(TARGET), TARGET);
    for _ in 0..WORK_RETRY_EXTERNAL_BUDGET {
        let scheduled = fixture.harness.stored(&admitted.id).await;
        now = now.max(stamp_time(&scheduled.work.ready_at));
        assert!(worker.run_one(&admitted.id, now).await.unwrap());
    }

    let scheduled = fixture.harness.stored(&admitted.id).await;
    now = now.max(stamp_time(&scheduled.work.ready_at));
    let expires = now + Duration::from_secs(60);
    let first_claim = fixture
        .harness
        .store
        .claim_sandbox_work(
            &admitted.id,
            "first-manager",
            rfc3339_at(now),
            rfc3339_at(expires),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_claim.work.attempts, WORK_RETRY_EXTERNAL_BUDGET);

    let pause = Arc::new(RetirementProbePause::default());
    *fixture
        .backend
        .nonexecuting_inspection_pause
        .lock()
        .unwrap() = Some(pause.clone());
    let settle = worker.run_claimed_for_test(first_claim.clone(), now);
    tokio::pin!(settle);
    tokio::select! {
        () = pause.entered.notified() => {}
        result = &mut settle => panic!("provider inspection did not wait: {result:?}"),
    }

    let takeover_at = expires + Duration::from_secs(1);
    let successor = fixture.harness.worker_with_runtime_image(
        "second-manager",
        fixture.adapter(TARGET),
        TARGET,
    );
    let winning_claim = fixture
        .harness
        .store
        .claim_sandbox_work(
            &admitted.id,
            "second-manager",
            rfc3339_at(takeover_at),
            rfc3339_at(takeover_at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(winning_claim.work.next_fence > first_claim.work.next_fence);
    pause.release.notify_one();
    assert_eq!(settle.await, Err(SandboxStoreError::Conflict));
    assert_eq!(fixture.harness.stored(&admitted.id).await, winning_claim);

    assert!(successor
        .run_claimed_for_test(winning_claim, takeover_at)
        .await
        .unwrap());
    let parked = fixture.harness.stored(&admitted.id).await;
    assert_eq!(parked.work.step, SandboxWorkStep::None);
    assert_eq!(parked.lifecycle.operation, admitted.lifecycle.operation);
    assert_eq!(
        parked.lifecycle.unpublished_recovery,
        admitted.lifecycle.unpublished_recovery
    );
    assert_eq!(parked.lifecycle.home, admitted.lifecycle.home);
    assert_eq!(
        parked.lifecycle.suspended_runtime,
        admitted.lifecycle.suspended_runtime
    );
    fixture.backend.nonexecuting.store(true, Ordering::SeqCst);
    now = takeover_at + Duration::from_secs(61);
    assert!(!successor.run_one(&admitted.id, now).await.unwrap());
    assert_eq!(fixture.harness.stored(&admitted.id).await, parked);
    assert_eq!(provider_suspends(&fixture), 0);

    let retried = retry_parked_update(&fixture, &parked).await;
    assert_eq!(retried.lifecycle.operation, parked.lifecycle.operation);
    now = now.max(stamp_time(&retried.work.ready_at));
    assert!(successor.run_one(&admitted.id, now).await.unwrap());
    let quiesced = fixture.harness.stored(&admitted.id).await;
    let state = quiesced.lifecycle.unpublished_recovery.as_ref().unwrap();
    let admitted_state = admitted.lifecycle.unpublished_recovery.as_ref().unwrap();
    assert_eq!(state.proof, admitted_state.proof);
    let receipt = state.quiescence.as_ref().unwrap();
    assert_eq!(receipt.operation_id, admitted_state.operation_id);
    assert_eq!(
        receipt.provider_plan_sha256,
        admitted_state.proof.provider_plan_sha256
    );
    assert_eq!(receipt.home_identity, admitted_state.proof.home_identity);
    assert_eq!(receipt.conversations, admitted_state.proof.conversations);
    assert_eq!(provider_suspends(&fixture), 0);

    fixture.backend.lose_suspend.store(true, Ordering::SeqCst);
    now = now.max(stamp_time(&quiesced.work.ready_at));
    assert!(successor.run_one(&admitted.id, now).await.unwrap());
    let awaiting_suspend_receipt = fixture.harness.stored(&admitted.id).await;
    assert_eq!(
        awaiting_suspend_receipt.lifecycle.operation,
        quiesced.lifecycle.operation
    );
    assert_eq!(
        awaiting_suspend_receipt.lifecycle.unpublished_recovery,
        quiesced.lifecycle.unpublished_recovery
    );
    assert_eq!(provider_suspends(&fixture), 1);
    let restored = fixture
        .drive(&admitted.id, &mut now, SandboxStatus::Active, TARGET)
        .await;
    assert_eq!(provider_suspends(&fixture), 1);
    assert_eq!(restored.lifecycle.home, admitted.lifecycle.home);
    assert_eq!(
        restored.runtime.as_ref().unwrap().runtime_id,
        parked
            .lifecycle
            .suspended_runtime
            .as_ref()
            .unwrap()
            .runtime_id
    );
    assert_eq!(restored.runtime.unwrap().provision_generation, Some(3));
    let recovered_workspace = fixture
        .harness
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered_workspace, workspace);
    let recovered = fixture.backend.snapshot().await;
    assert_eq!(recovered.homes, original.homes);
}

fn provider_suspends(fixture: &Fixture) -> usize {
    fixture
        .backend
        .calls
        .snapshot()
        .iter()
        .filter(|call| call.as_str() == "suspend")
        .count()
}

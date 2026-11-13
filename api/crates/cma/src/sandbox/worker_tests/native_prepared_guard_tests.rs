use super::*;
use crate::sandbox::native_prepared::{complete_step, prepare_step_with_proof};
use crate::store::native_prepared::NativePreparedWorkspaceProof;
use layer3_adapter::agent_sandbox::NativeClaimGuardReason;

async fn completed(
    harness: &Harness,
    pools: &Arc<Pools>,
) -> (WarmSandbox, NativePreparedWorkspaceProof) {
    let mut record = reservation(harness).await;
    let mut proof = None;
    let policy = layer3_adapter::HomeVolumePolicy {
        mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "storage".into(),
        capacity_gib: 64,
        template_name: String::new(),
        template_generation: String::new(),
    };
    for _ in 0..20 {
        (record, proof) = prepare_step_with_proof(
            adapter(harness, pools.clone()).as_ref(),
            "test",
            &record,
            &policy,
            proof,
        )
        .await
        .unwrap();
        record = persist(harness, record).await;
        if record.state == WarmSandboxState::Ready {
            let proof = proof.expect("actual workspace receipts must be retained");
            proof.validate(&record).unwrap();
            return (record, proof);
        }
    }
    panic!("native preparation did not finish");
}

fn guarded_errors() -> [AdapterError; 4] {
    [
        NativeClaimGuardReason::Pending,
        NativeClaimGuardReason::TimedOut,
        NativeClaimGuardReason::CrashLoop,
        NativeClaimGuardReason::Restarting,
    ]
    .map(AdapterError::NativeClaimNotReady)
}

#[tokio::test]
async fn native_prepared_claim_guard_retains_exact_authority_and_workspace_proof() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let (completed, proof) = completed(&harness, &pools).await;
    let calls = pools.calls.lock().unwrap().clone();
    let requests = harness.backend.requests().await.len();
    for state in [WarmSandboxState::Preparing, WarmSandboxState::Installing] {
        let mut original = completed.clone();
        original.state = state;
        original.validate().unwrap();
        for original_proof in [None, Some(&proof)] {
            for error in guarded_errors()
                .into_iter()
                .chain([AdapterError::Unavailable("pending".into())])
            {
                let (next, retained, delay) = complete_step(&original, original_proof, Err(error));
                assert_eq!(
                    next, original,
                    "all Claim/member/PVC and hierarchy authority must survive"
                );
                assert_eq!(retained.as_ref(), original_proof);
                assert_eq!(delay, 5);
                let home = next.home.as_ref().unwrap();
                let native = home.native_pool.as_ref().unwrap();
                assert_eq!(native.claim.as_ref().unwrap().uid, "claim-uid");
                assert_eq!(
                    native.claim.as_ref().unwrap().sandbox_name.as_deref(),
                    Some("warm-member")
                );
                assert_eq!(home.receipt.as_ref().unwrap().storage_generation, "pvc-uid");
                assert!(native.cleanup.is_none());
                assert!(!native.cleaned && !home.released);
                assert!(matches!(
                    retire_step(adapter(&harness, pools.clone()).as_ref(), &next).await,
                    Err(AdapterError::Rejected(_))
                ));
            }
        }
    }
    assert_eq!(*pools.calls.lock().unwrap(), calls);
    assert_eq!(harness.backend.requests().await.len(), requests);
}

#[tokio::test]
async fn native_prepared_claim_guard_keeps_ready_and_retiring_maintenance_policy() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let (original, proof) = completed(&harness, &pools).await;
    for state in [WarmSandboxState::Ready, WarmSandboxState::Retiring] {
        let mut original = original.clone();
        original.state = state;
        for error in guarded_errors()
            .into_iter()
            .chain([AdapterError::Unavailable("maintenance".into())])
        {
            let (mut next, retained, delay) = complete_step(&original, Some(&proof), Err(error));
            assert_eq!(next.state, WarmSandboxState::Retiring);
            assert!(retained.is_none());
            assert_eq!(delay, 30);
            next.state = original.state;
            assert_eq!(next, original);
        }
    }
    for state in [WarmSandboxState::Preparing, WarmSandboxState::Installing] {
        let mut original = original.clone();
        original.state = state;
        let (next, retained, delay) = complete_step(
            &original,
            Some(&proof),
            Err(AdapterError::Rejected("invalid authority".into())),
        );
        assert_eq!(next.state, WarmSandboxState::Retiring);
        assert_eq!(next.home, original.home);
        assert!(retained.is_none());
        assert_eq!(delay, 30);
    }
}

#[tokio::test]
async fn native_prepared_claim_guard_success_keeps_completed_effect_and_schedule() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let (original, proof) = completed(&harness, &pools).await;
    for (state, expected_delay) in [
        (WarmSandboxState::Installing, 0),
        (WarmSandboxState::Ready, 5),
    ] {
        let mut completed = original.clone();
        completed.state = state;
        let (next, retained, delay) = complete_step(
            &original,
            None,
            Ok((completed.clone(), Some(proof.clone()))),
        );
        assert_eq!(next, completed);
        assert_eq!(retained, Some(proof.clone()));
        assert_eq!(delay, expected_delay);
    }
}

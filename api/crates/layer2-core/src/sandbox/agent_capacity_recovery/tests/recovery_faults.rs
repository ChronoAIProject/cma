use super::recovery::enabled;
use super::recovery_store::{Fault, Runtime, Store};
use super::*;
use crate::sandbox::agent_capacity_recovery::ports::*;
use crate::sandbox::agent_capacity_recovery::state::*;

const EXPIRES: &str = "2026-09-05T00:32:01Z";

#[tokio::test(start_paused = true)]
async fn lease_expiring_during_reconnect_never_completes_or_acknowledges() {
    for unavailable in [false, true] {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime.clone());
        let caller = store.caller();
        let request = store.request();
        svc.admit(&caller, &request).await.unwrap();
        {
            let mut r = runtime.0.lock().unwrap();
            r.delay = std::time::Duration::from_secs(180);
            r.unavailable = unavailable;
        }
        assert_eq!(
            svc.resume(&caller, &request, "expired", NOW, LATER, EXPIRES)
                .await,
            Err(RecoveryError::Conflict)
        );
        let s = store.0.lock().unwrap();
        assert_eq!(s.fixture.agent.status, AgentStatus::Failed);
        assert_eq!(s.publications, 0);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    }
}

#[tokio::test]
async fn crash_after_native_confirmation_retries_same_target_without_input() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    let caller = store.caller();
    let request = store.request();
    svc.admit(&caller, &request).await.unwrap();
    let ClaimOutcome::Claimed(claim) = store
        .claim(&caller, &request, "crashed", NOW, LATER)
        .await
        .unwrap()
    else {
        panic!("claim");
    };
    let (_unacknowledged, _) = runtime.reattach_existing(&claim).await.unwrap();
    // A process crash loses the in-memory attachment before any publication.
    drop(claim);
    svc.resume(&caller, &request, "restarted", LATER, EXPIRES, EXPIRES)
        .await
        .unwrap();
    let r = runtime.0.lock().unwrap();
    assert_eq!(r.reconnects.len(), 2);
    assert_eq!(r.reconnects[0], r.reconnects[1]);
    assert_eq!(r.acknowledgements, 1);
    let s = store.0.lock().unwrap();
    assert_eq!(s.successors, 1);
    assert_eq!(s.publications, 1);
    assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
}

#[tokio::test]
async fn known_aborted_completion_retains_failed_state_until_exact_retry() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    let caller = store.caller();
    let request = store.request();
    svc.admit(&caller, &request).await.unwrap();
    store.0.lock().unwrap().fault = Fault::BeforeCommit;
    assert_eq!(
        svc.resume(&caller, &request, "first", NOW, LATER, LATER)
            .await
            .unwrap(),
        CommitOutcome::KnownAbort
    );
    assert_eq!(
        store.0.lock().unwrap().fixture.agent.status,
        AgentStatus::Failed
    );
    assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    svc.resume(&caller, &request, "retry", LATER, EXPIRES, EXPIRES)
        .await
        .unwrap();
    assert_eq!(store.0.lock().unwrap().publications, 1);
    assert_eq!(runtime.0.lock().unwrap().acknowledgements, 1);
}

#[tokio::test]
async fn parent_intent_changing_after_handshake_prevents_publication() {
    for retirement in [true, false] {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime.clone());
        let caller = store.caller();
        let request = store.request();
        svc.admit(&caller, &request).await.unwrap();
        let ClaimOutcome::Claimed(claim) = store
            .claim(&caller, &request, "holder", NOW, LATER)
            .await
            .unwrap()
        else {
            panic!("claim");
        };
        let (_, confirmation) = runtime.reattach_existing(&claim).await.unwrap();
        {
            let mut s = store.0.lock().unwrap();
            if retirement {
                s.fixture.sandbox.status = SandboxStatus::Retiring;
            } else {
                s.fixture.agent.request_delete(NOW.into()).unwrap();
            }
        }
        assert_eq!(
            store.complete(&caller, &claim, &confirmation, NOW).await,
            Err(RecoveryError::Disposition(
                CapacityRecoveryDisposition::CleanupOnly
            ))
        );
        assert_eq!(store.0.lock().unwrap().publications, 0);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    }
}

#[tokio::test]
async fn corrupt_work_is_rejected_before_native_reconnect() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    let caller = store.caller();
    let request = store.request();
    svc.admit(&caller, &request).await.unwrap();
    let original = store.0.lock().unwrap().work.clone().unwrap();
    for variant in 0..9 {
        let mut work = original.clone();
        match variant {
            0 => work.format_version += 1,
            1 => work.next_fence = 0,
            2 => work.target.original_open.step = AgentWorkStep::DispatchFirstMessage,
            3 => work.target.original_open.completed_at = EXPIRES.into(),
            4 => work.target.runtime.bound_at = "invalid".into(),
            5 => work.target.conversation_id = "x".repeat(513),
            6 => work.target.active_turn = Some(request.failure_seq + 1),
            7 => work.target.home_identity = "x".repeat(16 * 1024),
            _ => work.phase = RecoveryPhase::Attaching,
        }
        store.0.lock().unwrap().work = Some(work);
        assert_eq!(
            svc.resume(&caller, &request, "holder", NOW, LATER, EXPIRES)
                .await,
            Err(RecoveryError::InvalidEvidence),
            "variant {variant}"
        );
    }
    assert!(runtime.0.lock().unwrap().reconnects.is_empty());
    let mut value = serde_json::to_value(original).unwrap();
    value["unknown_authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RecoveryWork>(value).is_err());
}

#[tokio::test]
async fn waiting_approval_requires_matching_active_turn_and_original_receipts() {
    let mut fixture = Fixture::new();
    fixture.agent.conversation.active_turn = Some(5);
    let store = Store::new(fixture);
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    let caller = store.caller();
    let request = store.request();
    svc.admit(&caller, &request).await.unwrap();
    let ClaimOutcome::Claimed(claim) = store
        .claim(&caller, &request, "holder", NOW, LATER)
        .await
        .unwrap()
    else {
        panic!("claim");
    };
    let (_, mut confirmation) = runtime.reattach_existing(&claim).await.unwrap();
    confirmation.native_state = ConfirmedNativeState::WaitingApproval;
    store
        .complete(&caller, &claim, &confirmation, NOW)
        .await
        .unwrap();
    let s = store.0.lock().unwrap();
    assert_eq!(s.fixture.agent.status, AgentStatus::WaitingApproval);
    assert_eq!(s.fixture.agent.conversation.active_turn, Some(5));
    assert_eq!(s.fixture.agent.work.receipts, claim.agent.work.receipts);
}

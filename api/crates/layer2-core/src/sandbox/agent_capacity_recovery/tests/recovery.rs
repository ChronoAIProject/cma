use super::recovery_store::{Fault, Runtime, Store};
use super::*;
use crate::sandbox::agent_capacity_recovery::{ports::*, service::*, state::*};

const EXPIRES: &str = "2026-09-05T00:32:01Z";

pub(super) fn enabled(store: Store, runtime: Runtime) -> CapacityRecoveryService<Store, Runtime> {
    CapacityRecoveryService::new(store, runtime)
        .with_features(CapacityRecoveryFeatures { enabled: true })
}

#[tokio::test]
async fn default_off_and_independent_durable_gates_emit_nothing() {
    let store = Store::new(Fixture::new());
    let runtime = Runtime::default();
    let request = store.request();
    let caller = store.caller();
    let before = store.0.lock().unwrap().fixture.agent.clone();
    assert_eq!(
        CapacityRecoveryService::new(store.clone(), runtime.clone())
            .admit(&caller, &request)
            .await,
        Err(RecoveryError::Disabled)
    );
    for writer_gate in [false, true] {
        {
            let mut s = store.0.lock().unwrap();
            s.writer_gate = writer_gate;
            s.recovery_gate = !writer_gate;
        }
        assert_eq!(
            enabled(store.clone(), runtime.clone())
                .admit(&caller, &request)
                .await,
            Err(RecoveryError::Disabled)
        );
    }
    let s = store.0.lock().unwrap();
    assert_eq!(s.fixture.agent, before);
    assert!(s.work.is_none());
    assert_eq!(s.successors, 0);
    assert!(runtime.0.lock().unwrap().reconnects.is_empty());
}

#[tokio::test]
async fn absent_source_proof_is_unavailable_without_adoption() {
    let store = Store::new(Fixture::new());
    store.0.lock().unwrap().source_available = false;
    assert_eq!(
        enabled(store.clone(), Runtime::default())
            .admit(&store.caller(), &store.request())
            .await,
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(store.0.lock().unwrap().successors, 0);
}

#[tokio::test]
async fn exact_admission_retry_and_restart_never_create_two_successors() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let caller = store.caller();
    let runtime = Runtime::default();
    store.0.lock().unwrap().fault = Fault::BeforeCommit;
    assert_eq!(
        enabled(store.clone(), runtime.clone())
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::KnownAbort
    );
    assert_eq!(store.0.lock().unwrap().successors, 0);
    let original_logs = store.0.lock().unwrap().logs.clone();
    let first = enabled(store.clone(), runtime.clone())
        .admit(&caller, &request)
        .await
        .unwrap();
    for _ in 0..3 {
        let work = store.0.lock().unwrap().work.clone().unwrap();
        let persisted = serde_json::to_vec(&work).unwrap();
        store.0.lock().unwrap().work = Some(serde_json::from_slice(&persisted).unwrap());
        assert_eq!(
            enabled(store.clone(), runtime.clone())
                .admit(&caller, &request)
                .await
                .unwrap(),
            first
        );
    }
    let s = store.0.lock().unwrap();
    assert_eq!(s.successors, 1);
    assert_eq!(s.fixture.agent.conversation.last_seq, request.failure_seq);
    assert_eq!(s.fixture.agent.status, AgentStatus::Failed);
    assert_eq!(s.logs, original_logs);
}

#[tokio::test]
async fn unknown_admission_with_or_without_visible_rows_is_never_assumed_aborted() {
    for fault in [Fault::UnknownAbsent, Fault::AfterCommit] {
        let store = Store::new(Fixture::new());
        let request = store.request();
        let caller = store.caller();
        let runtime = Runtime::default();
        store.0.lock().unwrap().fault = fault;
        let svc = enabled(store.clone(), runtime.clone());
        assert_eq!(
            svc.admit(&caller, &request).await.unwrap(),
            CommitOutcome::Unknown
        );
        let successors = store.0.lock().unwrap().successors;
        assert_eq!(
            svc.admit(&caller, &request).await.unwrap(),
            CommitOutcome::Unknown
        );
        assert_eq!(
            svc.resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
                .await
                .unwrap(),
            CommitOutcome::Unknown
        );
        assert_eq!(store.0.lock().unwrap().successors, successors);
        assert!(runtime.0.lock().unwrap().reconnects.is_empty());
    }
}

#[tokio::test]
async fn gate_close_preserves_accepted_work_and_runtime_confirmation_precedes_ready() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let caller = store.caller();
    let runtime = Runtime::default();
    enabled(store.clone(), runtime.clone())
        .admit(&caller, &request)
        .await
        .unwrap();
    {
        let mut s = store.0.lock().unwrap();
        assert_eq!(s.fixture.agent.status, AgentStatus::Failed);
        s.writer_gate = false;
        s.recovery_gate = false;
    }
    let disabled = CapacityRecoveryService::new(store.clone(), runtime.clone());
    let result = disabled
        .resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
        .await
        .unwrap();
    assert!(matches!(
        result,
        CommitOutcome::Recorded(RecoveryWork {
            phase: RecoveryPhase::Completed,
            ..
        })
    ));
    let s = store.0.lock().unwrap();
    assert_eq!(s.fixture.agent.status, AgentStatus::Ready);
    assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
    assert_eq!(
        s.fixture.agent.conversation.last_seq,
        request.failure_seq + 1
    );
    assert_eq!(s.publications, 1);
    assert_eq!(runtime.0.lock().unwrap().acknowledgements, 1);
}

#[tokio::test]
async fn crash_after_claim_waits_for_expiry_and_stale_holder_cannot_publish() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let caller = store.caller();
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    svc.admit(&caller, &request).await.unwrap();
    let ClaimOutcome::Claimed(old) = store
        .claim(&caller, &request, "old", NOW, LATER)
        .await
        .unwrap()
    else {
        panic!("claim");
    };
    svc.resume(&caller, &request, "new", NOW, EXPIRES, LATER)
        .await
        .unwrap();
    assert!(runtime.0.lock().unwrap().reconnects.is_empty());
    let ClaimOutcome::Claimed(new) = store
        .claim(&caller, &request, "new", LATER, EXPIRES)
        .await
        .unwrap()
    else {
        panic!("takeover");
    };
    assert!(new.work.lease.as_ref().unwrap().fence > old.work.lease.as_ref().unwrap().fence);
    let (_, confirmation) = runtime.reattach_existing(&old).await.unwrap();
    assert_eq!(
        store.complete(&caller, &old, &confirmation, LATER).await,
        Err(RecoveryError::Conflict)
    );
    let (_, confirmation) = runtime.reattach_existing(&new).await.unwrap();
    assert!(matches!(
        store
            .complete(&caller, &new, &confirmation, LATER)
            .await
            .unwrap(),
        CommitOutcome::Recorded(_)
    ));
}

#[tokio::test]
async fn crash_or_timeout_after_runtime_reconnect_retries_only_original_native_target() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let caller = store.caller();
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    svc.admit(&caller, &request).await.unwrap();
    runtime.0.lock().unwrap().unavailable = true;
    svc.resume(&caller, &request, "first", NOW, EXPIRES, LATER)
        .await
        .unwrap();
    assert_eq!(
        store.0.lock().unwrap().fixture.agent.status,
        AgentStatus::Failed
    );
    runtime.0.lock().unwrap().unavailable = false;
    enabled(store.clone(), runtime.clone())
        .resume(&caller, &request, "restarted", LATER, EXPIRES, EXPIRES)
        .await
        .unwrap();
    let r = runtime.0.lock().unwrap();
    assert_eq!(r.reconnects.len(), 2);
    assert_eq!(r.reconnects[0], r.reconnects[1]);
    assert_eq!(r.acknowledgements, 1);
}

#[tokio::test]
async fn lost_completion_ack_reconciles_original_receipt_after_later_head_and_stop() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let caller = store.caller();
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    svc.admit(&caller, &request).await.unwrap();
    store.0.lock().unwrap().fault = Fault::AfterCommit;
    assert_eq!(
        svc.resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
            .await
            .unwrap(),
        CommitOutcome::Unknown
    );
    assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    assert_eq!(
        svc.resume(&caller, &request, "holder", LATER, EXPIRES, EXPIRES)
            .await
            .unwrap(),
        CommitOutcome::Unknown
    );
    {
        let mut s = store.0.lock().unwrap();
        s.unresolved = false; // Simulated exact receipt after a majority barrier.
        s.fixture.agent.conversation.last_seq += 7;
        s.fixture.agent.status = AgentStatus::Stopped;
    }
    let result = svc
        .resume(&caller, &request, "other", LATER, EXPIRES, EXPIRES)
        .await
        .unwrap();
    assert!(matches!(
        result,
        CommitOutcome::Recorded(RecoveryWork {
            phase: RecoveryPhase::Completed,
            ..
        })
    ));
    assert_eq!(
        store.0.lock().unwrap().fixture.agent.status,
        AgentStatus::Stopped
    );
    assert_eq!(store.0.lock().unwrap().publications, 1);
    assert_eq!(runtime.0.lock().unwrap().reconnects.len(), 1);
}

#[tokio::test]
async fn retirement_and_owner_delete_supersede_without_reconnect() {
    for retirement in [true, false] {
        let store = Store::new(Fixture::new());
        let request = store.request();
        let caller = store.caller();
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime.clone());
        svc.admit(&caller, &request).await.unwrap();
        {
            let mut s = store.0.lock().unwrap();
            if retirement {
                s.fixture.sandbox.status = SandboxStatus::Retiring;
            } else {
                s.fixture.agent.request_delete(NOW.into()).unwrap();
            }
        }
        let result = svc
            .resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
            .await
            .unwrap();
        assert!(matches!(
            result,
            CommitOutcome::Recorded(RecoveryWork {
                phase: RecoveryPhase::Superseded,
                ..
            })
        ));
        assert!(runtime.0.lock().unwrap().reconnects.is_empty());
        assert_eq!(
            store.0.lock().unwrap().fixture.agent.status,
            AgentStatus::Failed
        );
    }
}

#[tokio::test]
async fn unauthorized_or_changed_request_and_wrong_native_identity_fail_closed() {
    let store = Store::new(Fixture::new());
    let request = store.request();
    let mut caller = store.caller();
    let runtime = Runtime::default();
    let svc = enabled(store.clone(), runtime.clone());
    caller.subject = crate::SubjectId::new("unauthorized");
    assert_eq!(
        svc.admit(&caller, &request).await,
        Err(RecoveryError::Unauthorized)
    );
    let caller = store.caller();
    svc.admit(&caller, &request).await.unwrap();
    let mut changed = request.clone();
    changed.requested_at = LATER.into();
    assert_eq!(request.key().unwrap(), changed.key().unwrap());
    assert_eq!(
        svc.admit(&caller, &changed).await,
        Err(RecoveryError::Conflict)
    );
    runtime.0.lock().unwrap().mismatch = true;
    assert_eq!(
        svc.resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
            .await,
        Err(RecoveryError::InvalidEvidence)
    );
    assert_eq!(
        store.0.lock().unwrap().fixture.agent.status,
        AgentStatus::Failed
    );
    assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
}

#[tokio::test]
async fn queued_terminal_and_uncertain_input_facts_are_never_reopened_or_dispatched() {
    for phase in 0..7 {
        let mut f = Fixture::new();
        let mut first =
            FirstMessageRecord::new(&f.agent.creation_key, "fixture input", NOW).unwrap();
        match phase {
            1 => first.fail_startup(NOW.into()).unwrap(),
            2..=6 => {
                first.fence_dispatch_for(5, 6, 7, NOW.into()).unwrap();
                if phase >= 3 {
                    first.mark_write_confirmed(NOW.into()).unwrap();
                }
                if phase == 4 {
                    first.complete(NOW.into()).unwrap();
                } else if phase >= 5 {
                    let failure = crate::sandbox::FirstMessageFailure::new(
                        "ambiguous_write",
                        "fixture ambiguity",
                        false,
                        NOW,
                    )
                    .unwrap();
                    first.require_recovery(failure.clone()).unwrap();
                    if phase == 6 {
                        first.fail(failure).unwrap();
                    }
                    f.agent.conversation.active_turn = Some(5);
                } else {
                    f.agent.conversation.active_turn = Some(5);
                }
            }
            _ => {}
        }
        f.agent.conversation.first_message = Some(first.clone());
        let receipts = f.agent.work.receipts.clone();
        let store = Store::new(f);
        let request = store.request();
        let caller = store.caller();
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime);
        svc.admit(&caller, &request).await.unwrap();
        svc.resume(&caller, &request, "holder", NOW, EXPIRES, LATER)
            .await
            .unwrap();
        let s = store.0.lock().unwrap();
        assert_eq!(s.fixture.agent.conversation.first_message, Some(first));
        assert_eq!(s.fixture.agent.work.receipts, receipts);
        assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
        assert_eq!(s.logs.len(), 2); // Retained failure and real lifecycle completion only.
        assert!(!s.logs.iter().any(|e| matches!(
            e.kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )));
    }
}

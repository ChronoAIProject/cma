//! Completed activation restart/no-replay model tests. These exercise the
//! resumed protocol, not the still-Unsupported production native proof producer.
use super::recovery::enabled;
use super::recovery_store::{Fault, Runtime, Store};
use super::*;
use crate::sandbox::agent_capacity_recovery::{ports::*, service::*, state::*};

const EXPIRES: &str = "2026-09-05T00:32:01Z";

#[tokio::test]
async fn live_completion_restart_activates_exact_receipt_without_replaying_input() {
    // Crash before runtime ack, lost commit ack, and failed runtime ack.
    for boundary in 0..3 {
        for active in [false, true] {
            let mut fixture = Fixture::new();
            let mut first =
                FirstMessageRecord::new(&fixture.agent.creation_key, "original input", NOW)
                    .unwrap();
            if active {
                first.fence_dispatch_for(5, 6, 7, NOW.into()).unwrap();
                first.mark_write_confirmed(NOW.into()).unwrap();
                fixture.agent.conversation.active_turn = Some(5);
            }
            fixture.agent.conversation.first_message = Some(first);
            let original = fixture.agent.clone();
            let original_parent = fixture.sandbox.clone();
            let store = Store::new(fixture);
            let runtime = Runtime::default();
            let svc = enabled(store.clone(), runtime.clone());
            let caller = store.caller();
            let request = store.request();
            svc.admit(&caller, &request).await.unwrap();
            if boundary == 0 {
                let ClaimOutcome::Claimed(claim) = store
                    .claim(&caller, &request, "crashed", NOW, LATER)
                    .await
                    .unwrap()
                else {
                    panic!("claim");
                };
                let (_attachment, confirmation) = runtime.reattach_existing(&claim).await.unwrap();
                store
                    .complete(&caller, &claim, &confirmation, NOW)
                    .await
                    .unwrap();
                // Crash here: durable state survives; attachment is not acknowledged.
            } else if boundary == 1 {
                store.0.lock().unwrap().fault = Fault::AfterCommit;
                assert_eq!(
                    svc.resume(&caller, &request, "crashed", NOW, LATER, EXPIRES)
                        .await
                        .unwrap(),
                    CommitOutcome::Unknown
                );
            } else {
                runtime.0.lock().unwrap().acknowledgement_unavailable = true;
                assert_eq!(
                    svc.resume(&caller, &request, "crashed", NOW, LATER, EXPIRES)
                        .await,
                    Err(RecoveryError::Unavailable)
                );
            }
            let (receipt, live_agent, logs) = {
                let mut s = store.0.lock().unwrap();
                // Simulate the adapter's exact durable-receipt reconciliation.
                // A visible row alone cannot clear an unknown commit in production.
                s.unresolved = false;
                s.writer_gate = false;
                s.recovery_gate = false;
                let receipt = s.work.clone().unwrap();
                let encoded = serde_json::to_vec(&receipt).unwrap();
                s.work = Some(serde_json::from_slice(&encoded).unwrap());
                (receipt, s.fixture.agent.clone(), s.logs.clone())
            };
            runtime.0.lock().unwrap().acknowledgement_unavailable = false;
            let restarted = CapacityRecoveryService::new(store.clone(), runtime.clone());
            for _ in 0..2 {
                assert_eq!(
                    restarted.admit(&caller, &request).await.unwrap(),
                    CommitOutcome::Recorded(receipt.clone())
                );
                assert_eq!(
                    restarted
                        .resume(&caller, &request, "restart", LATER, EXPIRES, EXPIRES)
                        .await
                        .unwrap(),
                    CommitOutcome::Recorded(receipt.clone())
                );
            }
            let s = store.0.lock().unwrap();
            assert_eq!(s.fixture.agent, live_agent);
            assert_eq!(s.fixture.sandbox, original_parent);
            assert_eq!(
                s.fixture.agent.status,
                if active {
                    AgentStatus::Working
                } else {
                    AgentStatus::Ready
                }
            );
            assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
            assert_eq!(s.fixture.agent.work.receipts, original.work.receipts);
            let mut conversation = s.fixture.agent.conversation.clone();
            conversation.last_seq = original.conversation.last_seq;
            assert_eq!(conversation, original.conversation);
            assert_eq!(s.logs, logs);
            assert_eq!(s.logs.len(), 2);
            assert!(matches!(
                s.logs[0].kind,
                AgentLogKind::WorkStepFailed { .. }
            ));
            assert!(matches!(s.logs[1].kind, AgentLogKind::StatusChanged { .. }));
            assert_eq!(s.successors, 1);
            assert_eq!(s.publications, 1);
            let r = runtime.0.lock().unwrap();
            assert_eq!(r.reconnects.len(), 2);
            assert_eq!(r.acknowledgement_attempts, 1 + usize::from(boundary == 2));
            assert_eq!(r.acknowledgements, 1);
            assert_eq!(r.activated.len(), 1);
            assert!(r.reconnects.iter().all(|target| target == &r.reconnects[0]));
        }
    }
}

#[tokio::test]
async fn unknown_completion_blocks_reconnect_until_exact_reconciliation() {
    for committed in [false, true] {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime.clone());
        let caller = store.caller();
        let request = store.request();
        svc.admit(&caller, &request).await.unwrap();
        store.0.lock().unwrap().fault = if committed {
            Fault::AfterCommit
        } else {
            Fault::UnknownAbsent
        };
        assert_eq!(
            svc.resume(&caller, &request, "first", NOW, LATER, EXPIRES)
                .await
                .unwrap(),
            CommitOutcome::Unknown
        );
        for _ in 0..2 {
            assert_eq!(
                svc.admit(&caller, &request).await.unwrap(),
                CommitOutcome::Unknown
            );
            assert_eq!(
                svc.resume(&caller, &request, "restart", LATER, EXPIRES, EXPIRES)
                    .await
                    .unwrap(),
                CommitOutcome::Unknown
            );
        }
        assert_eq!(runtime.0.lock().unwrap().reconnects.len(), 1);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
        assert_eq!(store.0.lock().unwrap().publications, usize::from(committed));
        // Exact outcome has now been reconciled, including positive abort
        // evidence in the absent case; this is not a missing-row inference.
        store.0.lock().unwrap().unresolved = false;
        let result = svc
            .resume(&caller, &request, "restart", LATER, EXPIRES, EXPIRES)
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
        assert_eq!(s.successors, 1);
        assert_eq!(s.publications, 1);
        assert_eq!(s.fixture.agent.work.step, AgentWorkStep::None);
        let r = runtime.0.lock().unwrap();
        assert_eq!(r.reconnects.len(), 2);
        assert!(r.reconnects.iter().all(|target| target == &r.reconnects[0]));
        assert_eq!(r.acknowledgements, 1);
    }
}

#[tokio::test]
async fn completion_from_another_attempt_cannot_acknowledge_this_attachment() {
    let corruptions: [fn(&mut RecoveryWork); 5] = [
        |w| w.revision += 1,
        |w| w.attempts += 1,
        |w| w.next_fence += 1,
        |w| w.ready_at = LATER.into(),
        |w| w.completed_at = Some(LATER.into()),
    ];
    for corrupt in corruptions {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let svc = enabled(store.clone(), runtime.clone());
        let caller = store.caller();
        let request = store.request();
        svc.admit(&caller, &request).await.unwrap();
        store.0.lock().unwrap().completion_response_fault = Some(corrupt);
        assert_eq!(
            svc.resume(&caller, &request, "holder", NOW, LATER, EXPIRES)
                .await,
            Err(RecoveryError::InvalidEvidence)
        );
        assert_eq!(store.0.lock().unwrap().publications, 1);
        assert_eq!(runtime.0.lock().unwrap().acknowledgement_attempts, 0);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 0);
    }
}

#[tokio::test]
async fn completed_restart_refuses_changed_parent_or_fence_before_native_io() {
    for changed_parent in [true, false] {
        let store = Store::new(Fixture::new());
        let runtime = Runtime::default();
        let service = enabled(store.clone(), runtime.clone());
        let caller = store.caller();
        let request = store.request();
        service.admit(&caller, &request).await.unwrap();
        service
            .resume(&caller, &request, "original", NOW, LATER, EXPIRES)
            .await
            .unwrap();
        {
            let mut s = store.0.lock().unwrap();
            if changed_parent {
                s.fixture.sandbox.status = SandboxStatus::Retiring;
            } else {
                s.fixture.agent.work.next_fence += 1;
            }
        }
        assert_eq!(
            service
                .resume(&caller, &request, "restart", LATER, EXPIRES, EXPIRES)
                .await,
            Err(RecoveryError::Conflict)
        );
        assert_eq!(runtime.0.lock().unwrap().reconnects.len(), 1);
        assert_eq!(runtime.0.lock().unwrap().acknowledgements, 1);
        assert_eq!(store.0.lock().unwrap().publications, 1);
    }
}

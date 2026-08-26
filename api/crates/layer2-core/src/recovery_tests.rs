//! What a session owner is promised, expressed as tests.
//!
//! The promise is short: a session you did not stop keeps working, a session
//! you stopped stays readable and can be picked up again, and neither of those
//! depends on any process staying alive. Everything here is a way of taking
//! that promise away — restarting the control plane, losing a sandbox, asking a
//! provider that will not answer — and checking it survives.
//!
//! "Restart" is expressible because a service shares nothing with its successor
//! except the store, so a second `service()` over the same store is a new
//! process in every way that matters here.

use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::RuntimeLossReason;

use crate::attach::AttachmentClaim;
use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::{
    RuntimeCandidate, SessionBoundary, SessionLeaseRequest, SessionOperation,
};
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::lease::Detached;
use crate::service::Deployment;
use crate::session::{RecoveryState, RuntimeLossCause, SessionStatus, StopReason};
use crate::store::{SessionRecord, SessionStore};
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, service, service_over, spec, user, FakeAdapter, FakeStore,
};

/// Runtime losses in one session's log, in sequence order.
async fn runtime_losses(
    store: &FakeStore,
    id: &SessionId,
) -> Vec<(u64, String, Option<RuntimeLossCause>)> {
    store
        .events_since(id, 0)
        .await
        .expect("the event log is readable")
        .into_iter()
        .filter_map(|stored| {
            let seq = stored.seq;
            match stored.into_known()?.kind {
                EventKind::RuntimeLost { runtime_id, cause } => Some((seq, runtime_id, cause)),
                _ => None,
            }
        })
        .collect()
}

async fn switch_to_test_candidate(
    store: &FakeStore,
    adapter: &FakeAdapter,
    id: &SessionId,
    token: &str,
) -> RuntimeCandidate {
    let record = store.get(id).await.unwrap().unwrap();
    let authority = store.runtime_authority(id).await.unwrap().unwrap();
    let request = SessionLeaseRequest::new(
        id.clone(),
        format!("test-holder-{token}"),
        SessionBoundary {
            status: record.session.status,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, token)
        .await
        .unwrap();
    let runtime_id = format!("runtime-{token}");
    adapter
        .running
        .lock()
        .await
        .insert(runtime_id.clone(), Some(id.0.clone()));
    adapter
        .generations
        .lock()
        .await
        .insert(runtime_id.clone(), reservation.generation);
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, &runtime_id)
        .await
        .unwrap();
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap();
    store.release_session_lease(&lease).await.unwrap();
    candidate
}

#[tokio::test]
async fn a_stale_attachment_is_superseded_before_reclaim_lease_release_completes() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("stale-attachment-release-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let mut attachment = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the original runtime is attached");
    svc.event_logs.drain_retained_cleanups(&created.id).await;

    switch_to_test_candidate(
        store.as_ref(),
        adapter.as_ref(),
        &created.id,
        "stale-attachment-successor",
    )
    .await;
    store.pause_next_session_lease_release();
    let mut reconciliation = tokio::spawn({
        let svc = Arc::clone(&svc);
        async move { svc.reconcile().await }
    });
    store.wait_for_paused_session_lease_release().await;

    assert!(
        !svc.entries.read().await.contains_key(&created.id),
        "the stale local entry is removed before its reclaim lease is released"
    );
    assert!(matches!(
        attachment.superseded.try_recv(),
        Ok(Detached::RuntimeReplaced)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut reconciliation)
            .await
            .is_err(),
        "the notification must arrive while durable lease release is still blocked"
    );

    store.resume_session_lease_release();
    let reclaimed = tokio::time::timeout(Duration::from_secs(2), reconciliation)
        .await
        .expect("reconciliation resumes after lease release")
        .expect("the reconciliation task completes");
    assert_eq!(reclaimed.readopted_sessions, 1);
}

async fn retire_for_interrupted_recovery(
    store: &FakeStore,
    id: &SessionId,
) -> (
    crate::coordination::SessionLease,
    crate::coordination::RuntimeAuthorityRetirement,
) {
    let record = store.get(id).await.unwrap().unwrap();
    let authority = store.runtime_authority(id).await.unwrap().unwrap();
    let request = SessionLeaseRequest::new(
        id.clone(),
        "interrupted-recovery-holder",
        SessionBoundary {
            status: record.session.status,
            authority,
        },
        SessionOperation::Recover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let retired = store
        .retire_runtime_authority(&mut lease, "interrupted-recovery-retirement")
        .await
        .unwrap();
    (lease, retired)
}

fn is_runtime_lost(_record: &SessionRecord, events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event.kind, EventKind::RuntimeLost { .. }))
}

#[tokio::test]
async fn a_running_session_survives_the_control_plane_restarting() {
    // The headline promise. Nothing the owner did ended this session, so the
    // deployment that replaced the process serving it must not either.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let original = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    // The process goes. The sandbox does not.
    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(
        recovered.readopted_sessions, 1,
        "the running session must be taken back over, not started again"
    );
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .runtime_id,
        Some(original),
        "taking a session back over must not change the sandbox it is using"
    );
    assert!(
        adapter.terminated.lock().await.is_empty(),
        "nothing about a restart justifies tearing a live session down"
    );
}

#[tokio::test]
async fn a_session_whose_sandbox_stopped_serving_is_given_another() {
    /*
     * The production incident this was written for.
     *
     * `sesn_7e8fab0b…` had its container OOM-killed. The sandbox object stayed
     * — a container exiting does not remove it — so the provider went on
     * listing it, the census counted it as alive, reconciliation found record
     * and reality agreeing, and did nothing. The session reported `running`
     * with `recovery_state: normal` for nineteen minutes while its runtime was
     * a corpse, and its owner could not interrupt it because there was nothing
     * left to interrupt.
     *
     * The distinction from `a_session_whose_sandbox_vanished_is_given_another`
     * is exactly the bug: that one removes the handle, and reconciliation
     * always handled it. This one keeps the handle and stops it serving.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    // Still listed, no longer serving.
    adapter.stop_serving(&dead).await;
    assert!(
        adapter.running.lock().await.contains_key(&dead),
        "the sandbox must still be listed, or this is testing the wrong failure"
    );

    let recovered = first.reconcile().await;
    assert_eq!(
        recovered.reprovisioned_sessions, 1,
        "a sandbox that has stopped serving is gone, however present it looks"
    );

    let settled = await_settled(&first, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "and the session comes back usable rather than being left to rot"
    );
}

#[tokio::test]
async fn non_serving_compatibility_runtime_teardown_failure_defers_loss_and_replacement() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("non-serving-cleanup-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let h1 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H1 is durable");
    drop(first);

    adapter.stop_serving(&h1).await;
    adapter.fail_terminations(1);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let deferred = restarted.reconcile().await;

    assert_eq!(deferred.deferred_sessions, 1);
    assert_eq!(deferred.lost_runtimes, 0);
    assert_eq!(deferred.reprovisioned_sessions, 0);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .as_deref(),
        Some(h1.as_str()),
        "H1 remains durable while its provider teardown can be retried"
    );
    assert!(adapter.running.lock().await.contains_key(&h1));
    assert!(adapter.terminated.lock().await.is_empty());
    assert!(runtime_losses(&store, &created.id).await.is_empty());

    let recovered = restarted.reconcile().await;
    assert_eq!(recovered.lost_runtimes, 1);
    assert_eq!(recovered.reprovisioned_sessions, 1);
    let settled = await_settled(&restarted, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    let h2 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H2 is durable");
    assert_ne!(h2, h1);
    assert!(!adapter.running.lock().await.contains_key(&h1));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&h1)
    );
}

#[tokio::test]
async fn a_runtime_lost_while_no_process_was_serving_is_still_recorded() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    drop(first);
    adapter
        .stop_serving_because(&dead, RuntimeLossReason::OutOfMemory)
        .await;
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let moved = restarted.reconcile().await;

    assert_eq!(moved.lost_runtimes, 1);
    let losses = runtime_losses(&store, &created.id).await;
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].1, dead);
    assert_eq!(losses[0].2, Some(RuntimeLossCause::OutOfMemory));
}

#[tokio::test]
async fn a_runtime_lost_under_the_process_serving_it_is_recorded_too() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let running = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = running
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&running, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    adapter
        .stop_serving_because(&dead, RuntimeLossReason::Failed)
        .await;
    running.reconcile().await;

    let losses = runtime_losses(&store, &created.id).await;
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].1, dead);
    assert_eq!(losses[0].2, Some(RuntimeLossCause::Failed));
}

#[tokio::test]
async fn ambiguous_lost_runtime_retirement_removes_the_local_input_channel() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("ambiguous-recovery-owner");
    let running = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = running
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&running, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a runtime");
    adapter.terminal_inputs.lock().await.clear();
    adapter.stop_serving(&dead).await;
    store.lose_runtime_retirement_responses(2);

    let sweep = running.reconcile().await;
    assert_eq!(sweep.reprovisioned_sessions, 0);
    assert_eq!(sweep.deferred_sessions, 1);
    assert_eq!(
        store.get(&created.id).await.unwrap().unwrap().runtime_id,
        None,
        "retirement committed even though both responses were lost"
    );
    assert!(!running.entries.read().await.contains_key(&created.id));

    assert!(running
        .send_input(
            &alice,
            &created.id,
            b"never reach the retired runtime".to_vec()
        )
        .await
        .is_err());
    assert!(adapter.terminal_inputs.lock().await.is_empty());

    let resumed = running.reconcile().await;
    assert_eq!(resumed.lost_runtimes, 1);
    assert_eq!(resumed.reprovisioned_sessions, 1);
    let settled = await_settled(&running, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_ne!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .as_deref(),
        Some(dead.as_str()),
        "the next sweep resumes from exact debt and installs a replacement"
    );
}

#[tokio::test]
async fn recovery_confirms_a_hidden_provisioning_lease_advance() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("hidden-recovery-advance-owner");
    let running = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = running
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&running, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a runtime");
    adapter.lose(&dead).await;
    store.lose_runtime_advance_responses(1);

    let recovered = running.reconcile().await;
    assert_eq!(recovered.lost_runtimes, 1);
    assert_eq!(
        recovered.reprovisioned_sessions, 1,
        "the committed recovery boundary must continue despite its lost response"
    );

    let settled = await_settled(&running, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert!(
        store.active_session_lease(&created.id).await.is_none(),
        "the confirmed target boundary must be released after replacement"
    );
    assert_ne!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_id
            .as_deref(),
        Some(dead.as_str())
    );
}

#[tokio::test]
async fn recovery_resumes_when_a_process_dies_immediately_after_authority_retirement() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("retirement-crash-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .unwrap();
    adapter.lose(&dead).await;
    drop(first);

    let (lease, retired) = retire_for_interrupted_recovery(&store, &created.id).await;
    assert_eq!(retired.cleanup.runtime_id, dead);
    store.release_session_lease(&lease).await.unwrap();
    assert!(store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .is_none());

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let moved = restarted.reconcile().await;
    assert_eq!(moved.lost_runtimes, 1);
    assert_eq!(moved.reprovisioned_sessions, 1);
    let losses = runtime_losses(&store, &created.id).await;
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].1, dead);
    assert_eq!(
        await_settled(&restarted, &alice, &created.id).await.status,
        SessionStatus::Ready
    );
}

#[tokio::test]
async fn recovery_resumes_when_a_process_dies_after_recording_provisioning() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("provisioning-crash-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .unwrap();
    adapter.lose(&dead).await;
    drop(first);

    let (mut lease, retired) = retire_for_interrupted_recovery(&store, &created.id).await;
    let mut provisioning = store.get(&created.id).await.unwrap().unwrap();
    provisioning.session.status = SessionStatus::Provisioning;
    provisioning.session.recovery_state = RecoveryState::Recovering;
    provisioning.session.stop_reason = None;
    let loss_seq = provisioning.session.last_seq + 1;
    provisioning.session.last_seq = loss_seq;
    store
        .commit_session_idempotent(
            "interrupted-recovery-loss",
            &provisioning,
            &[Event {
                session_id: created.id.clone(),
                seq: loss_seq,
                kind: EventKind::RuntimeLost {
                    runtime_id: dead.clone(),
                    cause: None,
                },
                appended_at: String::new(),
            }],
        )
        .await
        .unwrap();
    provisioning = store.get(&created.id).await.unwrap().unwrap();
    let boundary = SessionBoundary::for_record(&provisioning, retired.cleanup.generation);
    store
        .advance_session_lease(&mut lease, &boundary)
        .await
        .unwrap();
    store.release_session_lease(&lease).await.unwrap();

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let moved = restarted.reconcile().await;
    assert_eq!(moved.lost_runtimes, 0, "the loss was already recorded");
    assert_eq!(moved.reprovisioned_sessions, 1);
    assert_eq!(runtime_losses(&store, &created.id).await.len(), 1);
    assert_eq!(
        await_settled(&restarted, &alice, &created.id).await.status,
        SessionStatus::Ready
    );
}

#[tokio::test]
async fn four_runtime_loss_outages_defer_both_the_loss_and_its_replacement() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_runtime_lost));
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("runtime-loss-outage-owner");
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let dead = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("ready record exists")
        .runtime_id
        .expect("ready record names its runtime");
    adapter.lose(&dead).await;

    store.fail_next(4);
    let rejected = svc.reconcile().await;
    assert_eq!(rejected.lost_runtimes, 0);
    assert_eq!(rejected.reprovisioned_sessions, 0);
    assert_eq!(rejected.deferred_sessions, 1);
    assert_eq!(store.matched_calls(), 4);
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a replacement cannot start without its RuntimeLost transaction"
    );
    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.runtime_id.as_deref(), Some(dead.as_str()));
    assert_eq!(durable.session.recovery_state, RecoveryState::Normal);
    assert!(runtime_losses(&inner, &created.id).await.is_empty());
    {
        let entries = svc.entries.read().await;
        let entry = entries
            .get(&created.id)
            .expect("a rejected loss does not remove live state");
        assert_eq!(
            entry.runtime.as_ref().map(|handle| handle.0.as_str()),
            Some(dead.as_str())
        );
    }

    let retried = svc.reconcile().await;
    assert_eq!(retried.lost_runtimes, 1);
    assert_eq!(retried.reprovisioned_sessions, 1);
    assert_eq!(store.matched_calls(), 5);
    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    let losses = runtime_losses(&inner, &created.id).await;
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].1, dead);
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the replacement begins only on the sweep that records the loss"
    );
}

#[tokio::test]
async fn a_lost_runtime_the_provider_said_nothing_about_still_records_the_loss() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    adapter.lose(&dead).await;
    first.reconcile().await;

    let losses = runtime_losses(&store, &created.id).await;
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].1, dead);
    assert_eq!(losses[0].2, None, "silence is not an unknown verdict");
}

#[tokio::test]
async fn the_loss_is_recorded_before_the_replacement_is_started() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let running = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = running
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&running, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    adapter.lose(&dead).await;
    running.reconcile().await;
    await_settled(&running, &alice, &created.id).await;

    let events = store
        .events_since(&created.id, 0)
        .await
        .expect("the event log is readable");
    let loss_seq = events
        .iter()
        .find_map(|event| {
            matches!(
                &event.kind,
                crate::event::StoredEventKind::Known(EventKind::RuntimeLost { .. })
            )
            .then_some(event.seq)
        })
        .expect("the loss is recorded");
    let replacement_ready = events
        .iter()
        .find_map(|event| match &event.kind {
            crate::event::StoredEventKind::Known(EventKind::StatusChanged {
                status: SessionStatus::Ready,
                ..
            }) if event.seq > loss_seq => Some(event.seq),
            _ => None,
        })
        .expect("the replacement becomes ready");
    assert!(loss_seq < replacement_ready);
}

#[tokio::test]
async fn a_session_that_never_had_a_runtime_records_no_loss() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    let old = record.runtime_id.take().expect("had a runtime");
    record.session.status = SessionStatus::Provisioning;
    store.seed(&record).await.expect("the fixture is updated");
    adapter.lose(&old).await;
    drop(first);

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted.reconcile().await;

    assert!(runtime_losses(&store, &created.id).await.is_empty());
}

#[tokio::test]
async fn a_runtime_that_stays_lost_is_recorded_once() {
    let store = Arc::new(FakeStore::default());
    let original = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&original));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(first);

    let unavailable = Arc::new(FakeAdapter {
        refuse_provision: true,
        ..FakeAdapter::default()
    });
    unavailable
        .running
        .lock()
        .await
        .insert(dead.clone(), Some(created.id.0.clone()));
    unavailable
        .stop_serving_because(&dead, RuntimeLossReason::Displaced)
        .await;
    let restarted = service(Arc::clone(&store), Arc::clone(&unavailable));

    restarted.reconcile().await;
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .runtime_id,
        None
    );
    restarted.reconcile().await;
    assert_eq!(runtime_losses(&store, &created.id).await.len(), 1);
}

#[tokio::test]
async fn a_session_being_restored_is_not_reported_as_normal() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let running = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = running
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&running, &alice, &created.id).await;
    let dead = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter.lose(&dead).await;
    adapter
        .fail_first
        .store(3, std::sync::atomic::Ordering::SeqCst);

    running.reconcile().await;
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .session
            .recovery_state,
        RecoveryState::Recovering
    );
    let settled = await_settled(&running, &alice, &created.id).await;
    assert_eq!(settled.recovery_state, RecoveryState::Normal);
    let mut durable_state = None;
    for _ in 0..100 {
        durable_state = Some(
            store
                .get(&created.id)
                .await
                .expect("readable")
                .expect("present")
                .session
                .recovery_state,
        );
        if durable_state == Some(RecoveryState::Normal) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(durable_state, Some(RecoveryState::Normal));
}

#[tokio::test]
async fn a_session_whose_sandbox_vanished_is_given_another() {
    // A node is replaced and the sandbox goes with it. The owner did not ask
    // for that and should not have to notice it.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let lost = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    drop(first);
    adapter.lose(&lost).await;

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;
    assert_eq!(recovered.reprovisioned_sessions, 1);

    let settled = await_settled(&restarted, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that lost its sandbox must end up usable again, not failed"
    );
    let now = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    assert_ne!(
        now, lost,
        "it must be a new sandbox, and be recorded as one"
    );
}

#[tokio::test]
async fn recovery_keeps_the_session_the_owner_configured() {
    // Replacing the sandbox is invisible only if what comes back is the same
    // session. A recovered session running a different model would be a
    // different session wearing the same identifier.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    // The model is pinned where it now lives: on what the session is started
    // from, rather than on the request. The guarantee under test is unchanged —
    // whatever the session was started with must survive its sandbox.
    let pinned =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            model: Some("a-specific-model".to_string()),
            ..Default::default()
        });
    let chosen = spec();

    let first = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&pinned),
    );
    let created = first.create_session(&alice, chosen).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let lost = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");

    drop(first);
    adapter.lose(&lost).await;
    let restarted = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&pinned),
    );
    restarted.reconcile().await;
    await_settled(&restarted, &alice, &created.id).await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(
        record.session.options.model.as_deref(),
        Some("a-specific-model"),
        "the options the owner chose must outlive the sandbox they were chosen for"
    );
    assert_eq!(record.session.id, created.id, "and so must the identity");
}

#[tokio::test]
async fn a_runtime_another_control_plane_created_is_left_alone() {
    // A sandbox provider is shared: one key sees everything provisioned against
    // it. A second control plane — staging, mid-rollout, or one being verified
    // against production infrastructure — would otherwise read every session it
    // has never heard of as an orphan and tear it down on its first sweep.
    let store = Arc::new(FakeStore::default());
    let theirs = Arc::new(FakeAdapter {
        created_by: Some("some-other-control-plane".to_string()),
        ..FakeAdapter::default()
    });
    theirs
        .running
        .lock()
        .await
        .insert("their-runtime".to_string(), Some("sesn_theirs".to_string()));

    let ours = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&theirs),
        Deployment {
            identity: Some("ours".to_string()),
            adopts_untagged: true,
            ..Deployment::default()
        },
    );
    let recovered = ours.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 0,
        "a runtime belonging to another control plane must never be reclaimed"
    );
    assert!(
        theirs.terminated.lock().await.is_empty(),
        "and must certainly not be torn down"
    );
}

#[tokio::test]
async fn a_secondary_control_plane_leaves_unnamed_runtimes_alone() {
    // The runtimes that predate deployment names are the first deployment's.
    // A second one sharing the provider has no way to tell whether they should
    // exist, so it must not decide.
    let store = Arc::new(FakeStore::default());
    let provider = Arc::new(FakeAdapter::default());
    provider
        .running
        .lock()
        .await
        .insert("older-runtime".to_string(), Some("sesn_older".to_string()));

    let secondary = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&provider),
        Deployment {
            identity: Some("verification".to_string()),
            adopts_untagged: false,
            ..Deployment::default()
        },
    );
    let recovered = secondary.reconcile().await;

    assert_eq!(recovered.orphaned_runtimes, 0);
    assert!(provider.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn an_unversioned_orphan_is_not_deleted_by_handle_alone() {
    // Ownership of the provider is not identity proof. A legacy runtime has no
    // generation to compare atomically, and its opaque handle may have been
    // reused since the census was taken.
    let store = Arc::new(FakeStore::default());
    let provider = Arc::new(FakeAdapter::default());
    provider
        .running
        .lock()
        .await
        .insert("stray".to_string(), Some("sesn_nobody".to_string()));

    let primary = crate::test_support::service_as(
        Arc::clone(&store),
        Arc::clone(&provider),
        Deployment {
            identity: Some("ours".to_string()),
            adopts_untagged: true,
            ..Deployment::default()
        },
    );
    let recovered = primary.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 0,
        "legacy metadata cannot authorise a destructive provider mutation"
    );
    assert!(provider.running.lock().await.contains_key("stray"));
    assert!(provider.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_recorded_cleanup_debt_is_reclaimed_under_fresh_authority() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let original = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .unwrap();
    drop(first);

    let retired = switch_to_test_candidate(&store, &adapter, &created.id, "generation-one").await;
    let current = switch_to_test_candidate(&store, &adapter, &created.id, "generation-two").await;
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.orphaned_runtimes, 2);
    assert!(!adapter.running.lock().await.contains_key(&original));
    assert!(!adapter
        .running
        .lock()
        .await
        .contains_key(&retired.runtime_id));
    assert!(adapter
        .running
        .lock()
        .await
        .contains_key(&current.runtime_id));
    assert!(store
        .runtime_cleanup_debts(&created.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn coordinated_readoption_refuses_to_attach_without_a_durable_lease() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let record = store.get(&created.id).await.unwrap().unwrap();
    let authority = store.runtime_authority(&created.id).await.unwrap().unwrap();
    let current_runtime = record.runtime_id.clone().unwrap();
    let current_generation = authority.generation;
    let rival = SessionLeaseRequest::new(
        created.id.clone(),
        "rival-readoption-holder",
        SessionBoundary {
            status: record.session.status,
            authority,
        },
        SessionOperation::Readopt,
        Duration::from_secs(30),
    );
    let _held = store.acquire_session_lease(&rival).await.unwrap();
    let channels_before = adapter.senders.lock().await.len();
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.readopted_sessions, 0);
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert_eq!(recovered.deferred_sessions, 1);
    assert_eq!(adapter.senders.lock().await.len(), channels_before);
    assert!(adapter.running.lock().await.contains_key(&current_runtime));
    assert_eq!(
        store
            .runtime_authority(&created.id)
            .await
            .unwrap()
            .unwrap()
            .generation,
        current_generation
    );
}

#[tokio::test]
async fn legacy_cleanup_requires_the_irreversible_old_writer_drain_proof() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let legacy = store.get(&created.id).await.unwrap().unwrap();
    let legacy_runtime = legacy.runtime_id.clone().unwrap();
    store.seed(&legacy).await.expect("seed legacy authority");
    adapter.generations.lock().await.remove(&legacy_runtime);
    drop(first);

    let current = switch_to_test_candidate(&store, &adapter, &created.id, "generation-one").await;
    let before_gate = service(Arc::clone(&store), Arc::clone(&adapter));
    assert_eq!(before_gate.reconcile().await.orphaned_runtimes, 0);
    assert!(adapter.running.lock().await.contains_key(&legacy_runtime));
    assert_eq!(
        store.runtime_cleanup_debts(&created.id).await.unwrap()[0].generation,
        0
    );

    store.authorize_legacy_cleanup();
    let after_gate = service(Arc::clone(&store), Arc::clone(&adapter));
    assert_eq!(after_gate.reconcile().await.orphaned_runtimes, 1);
    assert!(!adapter.running.lock().await.contains_key(&legacy_runtime));
    assert!(adapter
        .running
        .lock()
        .await
        .contains_key(&current.runtime_id));
    assert!(store
        .runtime_cleanup_debts(&created.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn delayed_cleanup_cannot_delete_a_handle_reused_at_the_same_generation() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let original = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .unwrap();
    drop(first);

    let retired = switch_to_test_candidate(&store, &adapter, &created.id, "generation-one").await;
    let current = switch_to_test_candidate(&store, &adapter, &created.id, "generation-two").await;
    adapter
        .running
        .lock()
        .await
        .insert(retired.runtime_id.clone(), Some("sesn_other".into()));

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.orphaned_runtimes, 1);
    assert!(adapter
        .running
        .lock()
        .await
        .contains_key(&retired.runtime_id));
    assert!(adapter
        .running
        .lock()
        .await
        .contains_key(&current.runtime_id));
    assert_eq!(adapter.terminated.lock().await.as_slice(), &[original]);
    assert!(store
        .runtime_cleanup_debts(&created.id)
        .await
        .unwrap()
        .iter()
        .any(|debt| {
            debt.runtime_id == retired.runtime_id && debt.generation == retired.generation.get()
        }));
}

#[tokio::test]
async fn a_provider_that_cannot_be_asked_records_no_loss() {
    // The dangerous reading of silence. An enumeration that fails says nothing
    // about what is running, and treating it as "nothing is running" would tear
    // down or replace every live session at once.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let mute = Arc::new(FakeAdapter {
        refuse_list: true,
        ..FakeAdapter::default()
    });
    // The sandbox exists as far as attaching is concerned; only the question
    // "what are you running" goes unanswered.
    let handle = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    mute.running
        .lock()
        .await
        .insert(handle, Some(created.id.0.clone()));

    let restarted = service(Arc::clone(&store), Arc::clone(&mute));
    let recovered = restarted.reconcile().await;

    assert_eq!(
        recovered.orphaned_runtimes, 0,
        "a provider that cannot be asked must not have its runtimes reclaimed"
    );
    assert_eq!(
        recovered.reprovisioned_sessions, 0,
        "nor its sessions replaced"
    );
    assert_eq!(
        recovered.readopted_sessions, 1,
        "attaching answers what enumeration could not, so the session comes back"
    );
    assert!(
        runtime_losses(&store, &created.id).await.is_empty(),
        "not knowing the provider state is not evidence of a loss"
    );
}

#[tokio::test]
async fn a_session_being_started_is_left_alone() {
    // Bring-up is detached and slow, so a session in the middle of starting
    // looks exactly like one whose start-up died. Acting on that would hand one
    // session two sandboxes.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    // Deliberately without waiting: the claim is held from before the record is
    // written until bring-up ends.
    svc.begin_start(&created.id).await;
    let recovered = svc.reconcile().await;

    assert_eq!(
        recovered.reprovisioned_sessions, 0,
        "a session already being started must not be started again"
    );
    svc.finish_start(&created.id).await;
}

#[tokio::test]
async fn stopping_keeps_the_session_and_releases_the_sandbox() {
    // Stopping is the reversible half of ending. The point is that the bill
    // stops without the work being lost.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("a running session can be stopped");

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(record.session.status, SessionStatus::Idle);
    assert_eq!(record.session.stop_reason, Some(StopReason::UserTerminated));
    assert!(
        !record.session.status.is_terminal(),
        "stopping must leave the session revivable"
    );
    assert_eq!(
        adapter.running_count().await,
        0,
        "the sandbox must actually be released"
    );

    let listed = svc
        .list_sessions(&alice, &crate::SessionListQuery::default())
        .await
        .expect("session list is readable");
    assert!(
        listed.items.iter().any(|s| s.id == created.id),
        "a stopped session must still be listed: stopping is not deleting"
    );
}

#[tokio::test]
async fn failed_compatibility_stop_cleanup_is_retried_after_restart() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("failed-stop-cleanup-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let h1 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H1 is durable");

    adapter.fail_terminations(1);
    let _ = first.stop_session(&alice, &created.id).await;
    let pending = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(pending.session.status, SessionStatus::Idle);
    assert_eq!(pending.runtime_id.as_deref(), Some(h1.as_str()));
    assert!(adapter.running.lock().await.contains_key(&h1));
    drop(first);

    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let cleaned = restarted.reconcile().await;
    assert_eq!(cleaned.deferred_sessions, 0);
    assert_eq!(cleaned.reprovisioned_sessions, 0);
    assert_eq!(cleaned.readopted_sessions, 0);
    let idle = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(idle.session.status, SessionStatus::Idle);
    assert_eq!(idle.runtime_id, None);
    assert!(!adapter.running.lock().await.contains_key(&h1));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&h1)
    );
}

#[tokio::test]
async fn revive_refuses_h2_while_compatibility_h1_cleanup_is_pending() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("pending-cleanup-revive-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let h1 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H1 is durable");

    adapter.fail_terminations(1);
    let _ = svc.stop_session(&alice, &created.id).await;
    let pending = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(pending.session.status, SessionStatus::Idle);
    assert_eq!(pending.runtime_id.as_deref(), Some(h1.as_str()));

    let error = svc
        .revive_session(&alice, &created.id)
        .await
        .expect_err("revive must wait for H1 cleanup");
    assert!(matches!(
        error,
        crate::error::CoreError::RuntimeUnavailable(_)
    ));
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "revive must not provision H2 while H1 remains durable"
    );
    assert_eq!(adapter.running_count().await, 1);
    assert!(adapter.running.lock().await.contains_key(&h1));
    let still_pending = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(still_pending.session.status, SessionStatus::Idle);
    assert_eq!(still_pending.runtime_id.as_deref(), Some(h1.as_str()));
}

#[tokio::test]
async fn a_stopped_session_is_not_given_a_sandbox_behind_the_owners_back() {
    // The controller starts anything the record says should be running. A
    // stopped session says the opposite, and a controller that could not tell
    // the difference would undo every stop within a sweep.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let recovered = svc.reconcile().await;
    assert_eq!(recovered.reprovisioned_sessions, 0);
    assert_eq!(recovered.readopted_sessions, 0);
    assert_eq!(
        adapter.running_count().await,
        0,
        "nothing may be running for a session its owner stopped"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .session
            .status,
        SessionStatus::Idle
    );
}

#[tokio::test]
async fn reviving_returns_the_same_session_to_use() {
    // What the owner gets back must be the session they stopped — same
    // identifier, same history — or "revive" would mean "start something else".
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let before = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let revived = svc
        .revive_session(&alice, &created.id)
        .await
        .expect("a stopped session can be revived");
    assert_eq!(
        revived.id, created.id,
        "reviving must not mint a new session"
    );

    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert!(
        settled.last_seq > before,
        "the history must continue rather than restart"
    );
    assert_eq!(adapter.running_count().await, 1);
}

#[tokio::test]
async fn reviving_confirms_a_hidden_provisioning_lease_advance() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("hidden-revive-advance-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let old_runtime = store
        .get(&created.id)
        .await
        .expect("the session is readable")
        .expect("the session remains durable")
        .runtime_id
        .expect("the original runtime is durable");
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    store.lose_runtime_advance_responses(1);
    svc.revive_session(&alice, &created.id)
        .await
        .expect("the committed revive boundary survives its lost response");

    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(adapter.running_count().await, 1);
    assert_ne!(
        store
            .get(&created.id)
            .await
            .expect("the revived session is readable")
            .expect("the revived session remains durable")
            .runtime_id
            .as_deref(),
        Some(old_runtime.as_str())
    );
    assert!(
        store.active_session_lease(&created.id).await.is_none(),
        "the confirmed revive boundary must be released after publication"
    );
}

#[tokio::test]
async fn a_deleted_session_cannot_be_revived() {
    // Delete is the one action with no way back, so it has to actually refuse.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.delete_session(&alice, &created.id)
        .await
        .expect("deletable");

    let revived = svc.revive_session(&alice, &created.id).await;
    assert!(
        revived.is_err(),
        "a session that no longer exists must not come back"
    );
}

#[tokio::test]
async fn reviving_someone_elses_session_is_refused() {
    // Recovery paths are still access-controlled paths. This one takes an
    // identifier and starts a runtime, so it is worth pinning explicitly.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");
    let mallory = user("mallory");
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    assert!(
        svc.revive_session(&mallory, &created.id).await.is_err(),
        "a session belongs to its owner, including when it is stopped"
    );
    assert!(
        svc.stop_session(&mallory, &created.id).await.is_err(),
        "and stopping is no more public than reviving"
    );
}

/// A store, a conversation-capable runtime, and the profile that puts a
/// session on the conversation plane — shared across two services, because
/// "restart" here means a second service over the same everything.
fn conversing_deployment() -> (
    Arc<FakeStore>,
    Arc<FakeAdapter>,
    Arc<crate::environment_service::EnvironmentProfileService>,
) {
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    (
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter {
            capabilities: crate::conversation_support::conversing(),
            ..FakeAdapter::default()
        }),
        profiles,
    )
}

#[tokio::test]
async fn a_conversation_session_is_taken_back_over_as_one() {
    // The §5 failure this closes, and the one that section did not anticipate:
    // not a deployment interrupting a session, but a deployment leaving it
    // listed, intact, and unusable. Readopt predates conversation mode and
    // attached a terminal to everything — which two of three kernels do not
    // have, and which leaves nothing to prompt even where it succeeds.
    let (store, adapter, profiles) = conversing_deployment();
    let alice = user("alice");

    let id = {
        let first = crate::test_support::service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&profiles),
        );
        let created = first.create_session(&alice, spec()).await.expect("created");
        await_settled(&first, &alice, &created.id).await;
        created.id
    };

    // The process is gone; the record and the runtime are not.
    let replacement = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&profiles),
    );
    // No revive: nothing was stopped. Prompting is what a person does next,
    // and it is where the runtime has to be picked back up — exactly as
    // attaching is for the terminal plane.
    replacement
        .send_prompt(&alice, &id, "still there?".to_string())
        .await
        .expect("a live runtime is taken back over, and the session prompted");
}

#[tokio::test]
async fn a_cross_replica_stop_fences_and_discards_the_retired_conversation() {
    let (store, adapter, profiles) = conversing_deployment();
    let alice = user("cross-replica-conversation-stop-owner");
    let stopping = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&profiles),
    );
    let created = stopping
        .create_session(&alice, spec())
        .await
        .expect("created");
    await_settled(&stopping, &alice, &created.id).await;
    let stale = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    assert_eq!(stale.reconcile().await.readopted_sessions, 1);

    let runtime = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("the session has a runtime");
    let generation = store
        .runtime_authority(&created.id)
        .await
        .unwrap()
        .unwrap()
        .generation;
    adapter.generations.lock().await.insert(
        runtime.clone(),
        std::num::NonZeroU64::new(generation + 1).unwrap(),
    );

    stopping
        .stop_session(&alice, &created.id)
        .await
        .expect("the first replica stops the session");
    assert!(stale.entries.read().await.contains_key(&created.id));
    assert!(adapter.running.lock().await.contains_key(&runtime));

    assert!(stale
        .send_prompt(&alice, &created.id, "never deliver".into())
        .await
        .is_err());
    let sent = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the stale conversation channel remains observable")
        .sent()
        .await;
    assert!(
        sent.is_empty(),
        "the retired channel received input: {sent:?}"
    );

    stale.reconcile().await;
    assert!(
        !stale.entries.read().await.contains_key(&created.id),
        "reconciliation removes stale entries even for stopped sessions"
    );
}

#[tokio::test]
async fn taking_a_conversation_back_over_rejoins_it_rather_than_opening_another() {
    // The half that is invisible without asking. The bridge in the pod
    // outlives a control plane restart, so opening a conversation rather than
    // rejoining one leaves a second running beside the first: the owner still
    // sees their history, because the durable log has it, and the agent has
    // lost everything it knew.
    let (store, adapter, profiles) = conversing_deployment();
    let alice = user("alice");

    let id = {
        let first = crate::test_support::service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&profiles),
        );
        let created = first.create_session(&alice, spec()).await.expect("created");
        await_settled(&first, &alice, &created.id).await;
        created.id
    };
    assert_eq!(
        adapter.rejoined.lock().await.as_slice(),
        &[None],
        "a session being started has no conversation to rejoin"
    );

    let replacement = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&profiles),
    );
    replacement
        .send_prompt(&alice, &id, "still there?".to_string())
        .await
        .expect("taken back over");

    let asked = adapter.rejoined.lock().await.clone();
    assert_eq!(
        asked.last().expect("a second open"),
        &Some("conv-fixture".to_string()),
        "the replacement must rejoin what the runtime is already running"
    );
}

#[tokio::test]
async fn a_session_recorded_before_capabilities_existed_is_settled_when_it_comes_back() {
    // The upgrade case, and the one that decides whether deploying the
    // capability model costs somebody a running session. A record written by
    // an older build has no granted set at all; read back, that is every
    // capability false — which every gate reads as a refusal, including the
    // one behind each keystroke.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    // Exactly what an older build left behind: everything else intact, the
    // granted set absent.
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.capabilities = Default::default();
    assert!(record.session.capabilities.unrecorded());
    store.seed(&record).await.expect("recorded");

    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.readopted_sessions, 1);
    let settled = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .capabilities;
    assert!(
        settled.terminal,
        "a terminal session must come back able to take a keystroke"
    );
    assert!(
        !settled.unrecorded(),
        "the settled set must be written down, not derived again by every process"
    );
    // Through the interface a caller actually uses, not only in the record.
    restarted
        .send_input(&alice, &created.id, b"ls\n".to_vec())
        .await
        .expect("a readopted session must accept input");
}

#[tokio::test]
async fn a_session_that_recorded_its_own_set_keeps_it() {
    // The other half: settling is for records that never had one. A session
    // that was granted its abilities deliberately must not have them
    // recomputed underneath it — that is the recomputation the whole model
    // exists to forbid.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    // A set that could never be derived from this runtime, so a recomputation
    // is visible rather than coincidental.
    record.session.capabilities = crate::capabilities::SessionCapabilities {
        terminal: true,
        snapshot: false,
        ..Default::default()
    };
    store.seed(&record).await.expect("recorded");

    drop(first);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    restarted.reconcile().await;

    let after = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .capabilities;
    assert!(
        after.terminal && !after.snapshot,
        "a recorded set must survive being picked back up: {after:?}"
    );
}

#[tokio::test]
async fn a_provider_that_answers_nothing_defers_rather_than_replaces() {
    // The failure that already happened in production: the provider could not
    // be enumerated *and* could not be attached to, because it was not
    // answering at all. Reading that as "the sandbox is gone" replaces a
    // session that is very probably still running, and the replacement carries
    // no conversation.
    let store = Arc::new(FakeStore::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let original = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(first);

    // A provider that answers neither question. Both failures have one cause,
    // which is exactly why one must not be read as evidence about the other.
    let silent = Arc::new(FakeAdapter {
        refuse_list: true,
        refuse_attach: true,
        ..FakeAdapter::default()
    });
    let restarted = service(Arc::clone(&store), Arc::clone(&silent));
    let recovered = restarted.reconcile().await;

    assert_eq!(
        recovered.reprovisioned_sessions, 0,
        "a session must not be replaced on evidence nobody has"
    );
    assert_eq!(
        recovered.deferred_sessions, 1,
        "it is deferred, and counted"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .runtime_id,
        Some(original),
        "the record must still name the sandbox that is still running"
    );
    assert!(
        silent.terminated.lock().await.is_empty(),
        "and nothing may be torn down on the same non-evidence"
    );
}

#[tokio::test]
async fn stale_compatibility_h1_teardown_failure_retains_its_local_cleanup_identity() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("stale-h1-cleanup-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let h1 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H1 is durable and installed");
    let h2 = format!("{h1}-successor");
    adapter
        .running
        .lock()
        .await
        .insert(h2.clone(), Some(created.id.0.clone()));
    let mut successor = store.get(&created.id).await.unwrap().unwrap();
    successor.runtime_id = Some(h2.clone());
    store
        .commit_session_idempotent("test-publish-compatibility-h2", &successor, &[])
        .await
        .expect("H2 becomes the durable runtime");

    adapter.fail_terminations(1);
    let deferred = svc.reconcile().await;
    assert_eq!(deferred.deferred_sessions, 1);
    assert_eq!(deferred.readopted_sessions, 0);
    assert_eq!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .and_then(|entry| entry.runtime.as_ref())
            .map(|runtime| runtime.0.as_str()),
        Some(h1.as_str()),
        "failed teardown retains the only local H1 cleanup identity"
    );
    assert!(adapter.running.lock().await.contains_key(&h1));
    assert!(adapter.running.lock().await.contains_key(&h2));
    assert!(adapter.terminated.lock().await.is_empty());

    let discarded = svc.reconcile().await;
    assert_eq!(discarded.deferred_sessions, 1);
    assert!(!svc.entries.read().await.contains_key(&created.id));
    assert!(!adapter.running.lock().await.contains_key(&h1));
    assert!(adapter.running.lock().await.contains_key(&h2));
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&h1)
    );

    let converged = svc.reconcile().await;
    assert_eq!(converged.readopted_sessions, 1);
    assert_eq!(
        svc.entries
            .read()
            .await
            .get(&created.id)
            .and_then(|entry| entry.runtime.as_ref())
            .map(|runtime| runtime.0.as_str()),
        Some(h2.as_str())
    );
}

#[tokio::test]
async fn failed_compatibility_readoption_retires_h1_before_provisioning_h2() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("failed-readoption-cleanup-owner");
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let h1 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H1 is durable");
    drop(first);

    adapter.fail_terminal_attaches(1);
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;
    assert_eq!(recovered.readopted_sessions, 0);
    assert_eq!(recovered.reprovisioned_sessions, 1);
    assert_eq!(recovered.deferred_sessions, 0);

    let settled = await_settled(&restarted, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    let h2 = store
        .get(&created.id)
        .await
        .unwrap()
        .unwrap()
        .runtime_id
        .expect("H2 is durable");
    assert_ne!(h2, h1);
    assert!(!adapter.running.lock().await.contains_key(&h1));
    assert!(adapter.running.lock().await.contains_key(&h2));
    assert_eq!(adapter.running_count().await, 1);
    assert_eq!(
        adapter.terminated.lock().await.as_slice(),
        std::slice::from_ref(&h1),
        "H1 is terminated before replacement publication"
    );
    assert_eq!(
        adapter
            .provision_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
}

#[tokio::test]
async fn a_runtime_the_provider_says_is_gone_is_still_replaced() {
    // The other half. Deferring is for uncertainty, not for a positive answer:
    // a session whose sandbox the provider says does not exist is one nobody
    // can use until it is given another.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    let handle = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(first);

    // The provider answers, and says this one is gone.
    adapter.lose(&handle).await;
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
    let recovered = restarted.reconcile().await;

    assert_eq!(recovered.reprovisioned_sessions, 1);
    assert_eq!(recovered.deferred_sessions, 0);
}

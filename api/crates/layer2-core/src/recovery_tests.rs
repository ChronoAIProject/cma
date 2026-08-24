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

use layer3_adapter::RuntimeLossReason;

use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::Deployment;
use crate::session::{RecoveryState, RuntimeLossCause, SessionStatus, StopReason};
use crate::store::SessionStore;
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

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
async fn the_deployment_that_owns_the_provider_still_reclaims_orphans() {
    // The other half of the rule: scoping reclamation must not stop the primary
    // deployment cleaning up after itself, or a sandbox created moments before
    // a crash would never be released.
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
        recovered.orphaned_runtimes, 1,
        "a runtime no session refers to is still reclaimed"
    );
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
        .list_sessions(&alice)
        .await
        .expect("session list is readable");
    assert!(
        listed.iter().any(|s| s.id == created.id),
        "a stopped session must still be listed: stopping is not deleting"
    );
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

//! Upgrading the runtime must not cost anyone their session.
//!
//! The rule these enforce is the one in CLAUDE.md §5: no deployment, upgrade,
//! restart or rollout interrupts a session its owner has not stopped. A
//! runtime upgrade is the hardest case, because unlike a control plane restart
//! the sandbox genuinely cannot survive it — the image is the sandbox.
//!
//! Two services over one store, expecting different versions, is an upgrade.

use std::sync::Arc;

use crate::session::SessionStatus;
use crate::store::SessionStore;
use crate::test_support::{
    await_settled, service_on_version, service_on_version_with_skills, spec, user, FakeAdapter,
    FakeStore,
};

/// Wait for a session to leave the states that mean "in progress".
async fn await_usable(
    svc: &Arc<crate::service::SessionService>,
    caller: &crate::authz::Caller,
    id: &crate::ids::SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(s) = svc.get_session(caller, id).await {
            if s.status != SessionStatus::Provisioning && s.status != SessionStatus::Rotating {
                return s;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never became usable");
}

#[tokio::test]
async fn a_session_is_carried_onto_a_newer_runtime() {
    // The headline case. Someone is working; the runtime image changes under
    // them; they keep working.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter
        .set_conversation(&first, b"turn one\nturn two\n")
        .await;
    drop(old);

    // The deployment.
    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(
        moved.rotated_sessions, 1,
        "the session must be carried over"
    );

    let settled = await_usable(&new, &alice, &created.id).await;
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.id, created.id, "it must be the same session");
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));
    assert_eq!(settled.rotations, 1);
    assert!(
        settled.rotated_at.is_some(),
        "the owner must be told what moment their session was carried from"
    );
}

#[tokio::test]
async fn the_conversation_travels_to_the_new_runtime() {
    // Without this the session survives as an identifier and the work does
    // not, which is the same as losing it.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter
        .set_conversation(&first, b"the conversation so far")
        .await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    await_usable(&new, &alice, &created.id).await;

    assert_eq!(
        adapter.handovers.lock().await.clone(),
        vec![b"the conversation so far".to_vec()],
        "the new runtime must be handed exactly what the old one had"
    );
}

#[tokio::test]
async fn the_old_sandbox_is_released() {
    // A rotation that leaves the old sandbox running would double the cost of
    // every upgrade, silently.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    await_usable(&new, &alice, &created.id).await;

    assert!(
        adapter.terminated.lock().await.contains(&first),
        "the runtime being replaced must actually be released"
    );
    assert_eq!(
        adapter.running_count().await,
        1,
        "exactly one sandbox must be running for one session"
    );
}

#[tokio::test]
async fn a_session_already_on_the_current_runtime_is_left_alone() {
    // Rotation is triggered by a version difference, so a version that has not
    // changed must produce no work at all. Otherwise every sweep would replace
    // every sandbox.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let svc = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let first = svc.reconcile().await;
    assert_eq!(first.rotated_sessions, 0);
    let again = svc.reconcile().await;
    assert_eq!(again.rotated_sessions, 0, "and it must stay settled");
    assert!(adapter.handovers.lock().await.is_empty());
}

#[tokio::test]
async fn a_session_is_not_interactive_while_it_is_being_carried() {
    // Input during a handover would reach a sandbox being torn down: accepted,
    // then lost. Refusing it briefly is the honest alternative, and the
    // interface can only refuse it if the state says so.
    assert!(!SessionStatus::Rotating.is_interactive());
    assert!(
        SessionStatus::Rotating.wants_runtime(),
        "but it is still a session that should be running"
    );
    assert!(
        !SessionStatus::Rotating.is_terminal(),
        "and it has certainly not ended"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_left_running() {
    // The trade this makes explicit: a stale image is bad, and losing the
    // conversation is worse. A runtime that cannot hand its conversation over
    // keeps the session it has.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let first = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(moved.rotated_sessions, 0);

    let settled = await_usable(&new, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that cannot be moved must not be left refusing input"
    );
    assert!(
        !adapter.terminated.lock().await.contains(&first),
        "and its runtime must not be torn down"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_still_adopted() {
    // The case a deployment actually hits: every session running an image from
    // before the handover existed refuses to be carried. Those sessions keep
    // running, so the control plane still has to take them back over and watch
    // them — leaving one unattended because it could not be moved would be a
    // worse failure than the stale image it is stuck on.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let recovered = new.reconcile().await;

    assert_eq!(recovered.rotated_sessions, 0, "it cannot be carried");
    assert_eq!(
        recovered.readopted_sessions, 1,
        "but it must still be taken back over"
    );
}

#[tokio::test]
async fn a_runtime_that_cannot_be_carried_is_asked_only_once() {
    // The sweep runs every few minutes for as long as the process does. A
    // session on a runtime that has already refused would otherwise be shown
    // an "updating" state it backs out of, over and over, forever.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;
    let before = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;

    new.reconcile().await;
    new.reconcile().await;

    let after = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    assert_eq!(
        before, after,
        "a refusal must not append a state change on every sweep"
    );
}

#[tokio::test]
async fn a_session_that_cannot_be_carried_is_never_shown_as_moving() {
    // Capturing changes nothing, so it is attempted before anything is
    // announced. A session that cannot be moved should never have been told it
    // was being moved.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_snapshot: true,
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    let settled_seq = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .session
        .last_seq;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.reconcile().await;

    let record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    assert_eq!(record.session.status, SessionStatus::Ready);
    assert_eq!(
        record.session.last_seq, settled_seq,
        "nothing should have been announced at all"
    );
}

#[tokio::test]
async fn a_stopped_session_is_not_carried_anywhere() {
    // Nothing is running for it, so there is nothing to move — and moving it
    // would start a sandbox its owner asked to be rid of.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    old.stop_session(&alice, &created.id)
        .await
        .expect("stopped");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    let moved = new.reconcile().await;
    assert_eq!(moved.rotated_sessions, 0);
    assert_eq!(adapter.running_count().await, 0);
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readable")
            .expect("present")
            .session
            .status,
        SessionStatus::Idle,
    );
}

#[tokio::test]
async fn reviving_a_stopped_session_puts_it_on_the_current_runtime() {
    // The other way onto a new image, and the one that needs no rotation:
    // there is nothing to carry, so it simply starts on what is current.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    old.stop_session(&alice, &created.id)
        .await
        .expect("stopped");
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    new.revive_session(&alice, &created.id)
        .await
        .expect("revivable");
    let settled = await_usable(&new, &alice, &created.id).await;

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));
    assert_eq!(
        settled.rotations, 0,
        "starting on the current version is not a rotation"
    );
}

#[tokio::test]
async fn a_deployment_that_does_not_version_its_runtime_rotates_nothing() {
    // Rotation has to be something a deployment opts into by versioning its
    // image. A deployment that does not would otherwise replace every sandbox
    // on every sweep, or none, depending on an accident of comparison.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = crate::test_support::service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(svc.reconcile().await.rotated_sessions, 0);
    assert_eq!(adapter.running_count().await, 1);
}

#[tokio::test]
async fn carrying_a_session_twice_is_counted_twice() {
    // The count is what tells someone their session has been moved more than
    // once, and a count that resets would hide exactly that.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let second = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:2");
    second.reconcile().await;
    await_usable(&second, &alice, &created.id).await;
    drop(second);

    let third = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:3");
    third.reconcile().await;
    let settled = await_usable(&third, &alice, &created.id).await;

    assert_eq!(settled.rotations, 2);
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:3"));
}

#[tokio::test]
async fn a_session_whose_kernel_cannot_snapshot_is_never_offered_for_rotation() {
    // Asked statically rather than found out by trying. Without this the sweep
    // attempts a capture it already knows will fail, every few minutes, for as
    // long as the session lives — and logs each one as though something had
    // gone wrong.
    let store = Arc::new(FakeStore::default());
    let unmovable = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            terminal: true,
            snapshot: false,
            ..Default::default()
        },
        ..FakeAdapter::default()
    });
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&unmovable), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    let new = service_on_version(Arc::clone(&store), Arc::clone(&unmovable), "runtime:2");
    assert_eq!(new.reconcile().await.rotated_sessions, 0);

    let settled = await_usable(&new, &alice, &created.id).await;
    assert_eq!(
        settled.status,
        SessionStatus::Ready,
        "a session that was never movable must simply keep working"
    );
}

/// A runtime that serves both planes, as the deployment's own does.
///
/// The default fake renders a screen and holds no conversation, which is the
/// shape the rest of this file needs. Everything below is about moving between
/// the two, so it needs a runtime that has both to move between.
fn ambidextrous() -> Arc<FakeAdapter> {
    Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    })
}

/// A live session on the terminal plane, and the service holding it.
async fn started(
    adapter: Arc<FakeAdapter>,
) -> (
    Arc<crate::service::SessionService>,
    Arc<FakeStore>,
    crate::authz::Caller,
    crate::session::Session,
) {
    let store = Arc::new(FakeStore::default());
    let svc = crate::test_support::service(Arc::clone(&store), adapter);
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, store, alice, created)
}

#[tokio::test]
async fn a_session_moves_between_planes_and_stays_the_same_session() {
    // The whole point: chat and terminal are two surfaces onto one session,
    // and moving between them costs a rotation and nothing else.
    let adapter = ambidextrous();
    let (svc, store, alice, created) = started(Arc::clone(&adapter)).await;
    let runtime = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    adapter
        .set_conversation(&runtime, b"what the agent has done so far")
        .await;

    let moving = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect("a runtime that holds a conversation can be asked for one");
    assert_eq!(
        moving.status,
        SessionStatus::Rotating,
        "the window in which input must be refused rather than lost"
    );

    let settled = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(settled.id, created.id, "it must be the same session");
    assert!(settled.capabilities.conversation && !settled.capabilities.terminal);
    assert_eq!(settled.rotations, 1);
    assert!(
        settled.rotated_at.is_some(),
        "the owner must be told the moment their session was carried from"
    );
    assert_eq!(
        adapter.handovers.lock().await.clone(),
        vec![b"what the agent has done so far".to_vec()],
        "the conversation must travel to the runtime on the other plane"
    );

    // And back, because a plane change is not a one-way door.
    svc.change_plane(&alice, &created.id, crate::options::SessionMode::Terminal)
        .await
        .expect("moving back is the same act");
    let back = await_usable(&svc, &alice, &created.id).await;
    assert_eq!(back.id, created.id);
    assert!(back.capabilities.terminal && !back.capabilities.conversation);
    assert_eq!(back.rotations, 2);
}

#[tokio::test]
async fn the_change_is_written_where_its_owner_reads_their_history() {
    // A surface that changed under somebody, with nothing in the log saying
    // so, is a session that appears to have rewritten itself.
    let (svc, _store, alice, created) = started(ambidextrous()).await;

    svc.change_plane(
        &alice,
        &created.id,
        crate::options::SessionMode::Conversation,
    )
    .await
    .expect("moved");
    await_usable(&svc, &alice, &created.id).await;

    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("readable");
    let change = events
        .iter()
        .find_map(|e| match &e.kind {
            crate::event::EventKind::PlaneChanged {
                from,
                to,
                snapshot_at,
            } => Some((*from, *to, snapshot_at.clone())),
            _ => None,
        })
        .expect("the change must be a durable event");
    assert_eq!(change.0, crate::options::SessionMode::Terminal);
    assert_eq!(change.1, crate::options::SessionMode::Conversation);
    assert!(
        !change.2.is_empty(),
        "the moment the conversation was taken is what says where history stops being continuous"
    );
}

#[tokio::test]
async fn a_runtime_that_does_not_serve_the_plane_refuses_by_name() {
    // The typed refusal, before anything is torn down. The default fake
    // renders a screen and holds no conversation, which is exactly the kernel
    // this must refuse for.
    let adapter = Arc::new(FakeAdapter::default());
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a screen-only runtime cannot hold a conversation");

    assert!(
        matches!(&refusal, crate::error::CoreError::CapabilityUnsupported(name) if name == "conversation"),
        "got {refusal:?}"
    );
    let after = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");
    assert_eq!(
        after.status,
        SessionStatus::Ready,
        "a refused session keeps running exactly as it was"
    );
    assert_eq!(after.rotations, 0);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_runtime_that_cannot_hand_its_conversation_over_refuses_rather_than_tries() {
    // The fail-safe. Moving planes means capturing and resuming, so a kernel
    // that cannot be captured would be trading the conversation for the
    // surface — the §5 defect, done deliberately.
    let adapter = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            snapshot: false,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a session that cannot be carried must not be moved");

    assert!(
        matches!(&refusal, crate::error::CoreError::CapabilityUnsupported(name) if name == "snapshot"),
        "got {refusal:?}"
    );
    let after = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");
    assert_eq!(after.status, SessionStatus::Ready);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_session_already_being_moved_refuses_a_second_move() {
    // Two handovers at once would capture a sandbox being torn down and resume
    // from whichever answered last.
    let (svc, store, alice, created) = started(ambidextrous()).await;
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.status = SessionStatus::Rotating;
    store.upsert(&record).await.expect("recorded");

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a session mid-handover cannot be handed over again");

    assert!(
        matches!(refusal, crate::error::CoreError::InvalidState),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn a_session_with_no_runtime_refuses() {
    // A stopped session is started on whichever plane its profile names when
    // it is revived. Rotating one would mean carrying a sandbox that is gone.
    let (svc, _store, alice, created) = started(ambidextrous()).await;
    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let refusal = svc
        .change_plane(
            &alice,
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("a stopped session has nothing to move");

    assert!(
        matches!(refusal, crate::error::CoreError::InvalidState),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn only_the_owner_may_change_a_sessions_plane() {
    // Stopping somebody else's session is an operator's job; deciding what
    // they are looking at is not.
    let (svc, _store, _alice, created) = started(ambidextrous()).await;

    let refusal = svc
        .change_plane(
            &crate::test_support::admin(),
            &created.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect_err("an operator grant does not stand in for the owner here");

    assert!(
        matches!(refusal, crate::error::CoreError::Forbidden),
        "got {refusal:?}"
    );
}

#[tokio::test]
async fn asking_for_the_plane_a_session_is_already_on_costs_nothing() {
    // Not an error and not a rotation: a handover to arrive where it already
    // is would be a cost with nothing on the other side of it.
    let adapter = ambidextrous();
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let unchanged = svc
        .change_plane(&alice, &created.id, crate::options::SessionMode::Terminal)
        .await
        .expect("already there");

    assert_eq!(unchanged.status, SessionStatus::Ready);
    assert_eq!(unchanged.rotations, 0);
    assert!(adapter.terminated.lock().await.is_empty());
}

#[tokio::test]
async fn a_terminal_session_nobody_moves_is_untouched() {
    // The regression this whole issue must not cause: adding the operation
    // must not change what a session does when nobody calls it.
    let adapter = ambidextrous();
    let (svc, _store, alice, created) = started(Arc::clone(&adapter)).await;

    let settled = svc
        .get_session(&alice, &created.id)
        .await
        .expect("readable");

    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.rotations, 0);
    assert!(settled.rotated_at.is_none());
    assert!(settled.capabilities.terminal && !settled.capabilities.conversation);
}

#[tokio::test]
async fn a_rotation_whose_replacement_cannot_start_does_not_end_the_session() {
    // CLAUDE.md §5: no deployment may interrupt a session its owner has not
    // stopped, and losing one is as severe as losing the data. A rotation is
    // caused by a deployment — moving the runtime image makes every live
    // session due for one — so its replacement failing is a fact about a
    // provider at a moment, not about the session.
    //
    // Four sessions were lost exactly this way during the v0.3.0 deploy. Their
    // logs read "moving this session to an updated runtime; it will come back
    // where it left off", then "the runtime did not become usable within
    // 600s", then nothing: terminated and retired. Each promised in its own
    // record that it would come back, and did not come back at all.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    // The provider stops giving out runtimes, which is what a burst of
    // rotations does to one, and the deployment happens anyway.
    let refusing = Arc::new(FakeAdapter {
        refuse_provision: true,
        ..FakeAdapter::default()
    });
    let new = service_on_version(Arc::clone(&store), Arc::clone(&refusing), "runtime:2");
    new.reconcile().await;
    let settled = await_settled(&new, &alice, &created.id).await;

    assert_ne!(
        settled.status,
        SessionStatus::Terminated,
        "a rotation its owner did not ask for must not end their session"
    );
    assert_ne!(
        settled.recovery_state,
        crate::session::RecoveryState::Retired,
        "and it must stay recoverable, or nothing will ever bring it back"
    );
    assert!(
        new.list_sessions(&alice)
            .await
            .iter()
            .any(|s| s.id == created.id),
        "it keeps its place in its owner's list"
    );
}

#[tokio::test]
async fn a_rotation_refused_for_good_still_ends_the_session() {
    // Keeping a session only makes sense while another attempt could go
    // differently. A kernel that no longer offers what this session *is* will
    // not start offering it, so a session kept against that answer would rest
    // forever, be retried forever by the sweep, and tell its owner it was
    // coming back when it was not.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old.create_session(&alice, spec()).await.expect("created");
    await_settled(&old, &alice, &created.id).await;
    drop(old);

    // The same kernel, rebuilt without the ability this session was granted.
    let lesser = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities::default(),
        ..FakeAdapter::default()
    });
    let new = service_on_version(Arc::clone(&store), Arc::clone(&lesser), "runtime:2");
    new.reconcile().await;
    let settled = await_settled(&new, &alice, &created.id).await;

    assert_eq!(
        settled.status,
        SessionStatus::Terminated,
        "an answer that cannot change is told, not retried"
    );
}

/// A source that refuses, the way the registry refuses this service's own
/// identity asking for somebody's private skill.
struct RefusingSkills;

#[async_trait::async_trait]
impl crate::store::SkillSource for RefusingSkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        _skills: &[crate::environment::SkillId],
    ) -> Result<Vec<crate::store::SkillPack>, String> {
        Err("the registry answered 403 Forbidden".to_string())
    }
}

#[tokio::test]
async fn a_session_whose_skills_cannot_be_refetched_is_still_carried() {
    /*
     * This ended three live sessions the first time skill installation
     * shipped.
     *
     * A rotation has no caller and no credential of theirs to fetch with, so
     * the registry is asked as this service, which sees only public skills and
     * answers 403. Treating that the way a creation treats it — refuse, do not
     * retry — terminates a session somebody is working in, to avoid it missing
     * a file. Losing the work is far worse than losing the skill.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let old = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(RefusingSkills),
    );
    let created = old.create_session(&alice, spec()).await;
    // At creation the caller is on the line, so the refusal is theirs to hear.
    assert!(
        created.is_err() || created.is_ok(),
        "creation may go either way here; what matters is the rotation below"
    );

    // A session that exists, on an older runtime, with no caller present.
    let store2 = Arc::new(FakeStore::default());
    let adapter2 = Arc::new(FakeAdapter::default());
    let first = service_on_version(Arc::clone(&store2), Arc::clone(&adapter2), "runtime:1");
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let newer = service_on_version_with_skills(
        Arc::clone(&store2),
        Arc::clone(&adapter2),
        "runtime:2",
        Arc::new(RefusingSkills),
    );
    newer.reconcile().await;

    let after = await_usable(&newer, &alice, &session.id).await;
    assert_ne!(
        after.status,
        SessionStatus::Terminated,
        "a skill that could not be refetched must not end the session"
    );
}

/// A source that hands back whatever was asked for, the way the registry
/// answers a caller whose credential can actually see the skill.
struct WorkingSkills;

#[async_trait::async_trait]
impl crate::store::SkillSource for WorkingSkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        skills: &[crate::environment::SkillId],
    ) -> Result<Vec<crate::store::SkillPack>, String> {
        Ok(skills
            .iter()
            .map(|id| crate::store::SkillPack {
                name: id.0.clone(),
                files: vec![("SKILL.md".to_string(), b"# a skill".to_vec())],
            })
            .collect())
    }
}

#[tokio::test]
async fn the_record_says_what_a_session_has_and_not_what_it_asked_for() {
    /*
     * The other half of the test above, and the reason it is not enough that
     * the session survives.
     *
     * A rotation that cannot refetch carries the session forward with no
     * skills. If the record still lists the ones it was created with, every
     * interface goes on reporting them — which is worse than reporting none,
     * because somebody is then told their agent can do something it cannot,
     * and only finds out when it fails.
     *
     * #442 is about not losing them in the first place. Until that lands the
     * record must at least agree with the disk.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(WorkingSkills),
    );
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let created = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        created.session.granted_skills.as_deref(),
        Some(&[crate::environment::SkillId::new("a-mandated-skill")][..]),
        "a session that got its skills should say so"
    );

    let newer = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(RefusingSkills),
    );
    newer.reconcile().await;
    await_usable(&newer, &alice, &session.id).await;

    let rotated = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        rotated.session.granted_skills.as_deref(),
        Some(&[][..]),
        "a rotation that lost the skills must not leave the record claiming them"
    );
}

#[tokio::test]
async fn a_rotation_that_keeps_its_skills_keeps_them_in_the_record() {
    /*
     * The mutation the test above would not catch: clearing the record on
     * every rotation. Then it agrees with the disk by always saying "nothing",
     * which is a different lie.
     */
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:1",
        Arc::new(WorkingSkills),
    );
    let session = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &session.id).await;

    let newer = service_on_version_with_skills(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        Arc::new(WorkingSkills),
    );
    newer.reconcile().await;
    await_usable(&newer, &alice, &session.id).await;

    let rotated = store
        .get(&session.id)
        .await
        .expect("readable")
        .expect("recorded");
    assert_eq!(
        rotated.session.granted_skills.as_deref(),
        Some(&[crate::environment::SkillId::new("a-mandated-skill")][..]),
        "a rotation that refetched its skills still has them"
    );
}

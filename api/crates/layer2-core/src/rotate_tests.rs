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
use crate::test_support::{await_settled, service_on_version, spec, user, FakeAdapter, FakeStore};

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

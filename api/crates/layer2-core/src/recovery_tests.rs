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

use std::collections::HashMap;
use std::sync::Arc;

use crate::ids::SessionId;
use crate::options::AgentOptions;
use crate::session::{SessionStatus, StopReason};
use crate::store::{ContextStore, SessionStore, StoreError};
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, credential_on, service, service_with_context, service_with_credentials, spec,
    user, write_screen, FakeAdapter, FakeStore,
};

/// Context held in memory, keyed the way a real object store keys it.
#[derive(Default)]
struct FakeContext {
    objects: tokio::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait::async_trait]
impl ContextStore for FakeContext {
    async fn put(&self, id: &SessionId, bytes: Vec<u8>) -> Result<String, StoreError> {
        let key = format!("transcripts/{}", id.0);
        self.objects.lock().await.insert(key.clone(), bytes);
        Ok(key)
    }

    async fn get(&self, reference: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.objects.lock().await.get(reference).cloned())
    }

    async fn delete(&self, reference: &str) -> Result<(), StoreError> {
        self.objects.lock().await.remove(reference);
        Ok(())
    }
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

    let mut chosen = spec();
    chosen.options = AgentOptions {
        model: Some("a-specific-model".to_string()),
        ..AgentOptions::default()
    };

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
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
    let restarted = service(Arc::clone(&store), Arc::clone(&adapter));
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
async fn a_provider_that_cannot_be_asked_changes_nothing() {
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

    let listed = svc.list_sessions(&alice).await;
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

#[tokio::test]
async fn a_stopped_session_can_still_be_read() {
    // The specific complaint this answers: stopping a session and finding the
    // conversation gone. The runtime is released; the transcript is not.
    let store = Arc::new(FakeStore::default());
    let context = Arc::new(FakeContext::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service_with_context(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    write_screen(&svc, &created.id, b"hello from the agent").await;

    svc.stop_session(&alice, &created.id)
        .await
        .expect("stopped");

    let transcript = svc
        .session_transcript(&alice, &created.id)
        .await
        .expect("a stopped session can still be read");
    assert_eq!(
        transcript, b"hello from the agent",
        "stopping a session must not throw away what it showed"
    );
}

#[tokio::test]
async fn what_a_session_showed_survives_the_process_that_showed_it() {
    // Coming back to a live session and finding it blank is indistinguishable
    // from losing the work, so the transcript has to outlive the process.
    let store = Arc::new(FakeStore::default());
    let context = Arc::new(FakeContext::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service_with_context(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    write_screen(&first, &created.id, b"work in progress").await;
    first.capture_transcripts().await;
    drop(first);

    let restarted = service_with_context(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    restarted.reconcile().await;

    let transcript = restarted
        .session_transcript(&alice, &created.id)
        .await
        .expect("readable after a restart");
    assert_eq!(
        transcript, b"work in progress",
        "a session taken back over must show what it showed before"
    );
}

#[tokio::test]
async fn opening_a_recovered_session_funds_it_there_and_then() {
    // The order this actually happens in: the control plane restarts and
    // recovers the session before anyone has signed back in, so there is no
    // credential to give it. Its owner then arrives. Waiting for a sweep to
    // notice would leave them with a session that opens, looks healthy, and
    // refuses every model call for minutes.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    // Recovered with no credential source at all, as happens when nobody has
    // signed in yet.
    let restarted =
        service_with_credentials(Arc::clone(&store), Arc::clone(&adapter), "arrived-later");
    restarted.reconcile().await;

    restarted
        .attach_terminal(&alice, &created.id, TerminalGeometry::default())
        .await
        .expect("the owner can open their session");

    assert_eq!(
        credential_on(&restarted, &created.id).await.as_deref(),
        Some("arrived-later"),
        "opening a session must hand it the credential its owner has now"
    );
}

#[tokio::test]
async fn a_recovered_session_is_given_its_owners_credential() {
    // A session recovered after a restart holds a credential belonging to a
    // process that no longer exists. Until it is replaced the agent can run but
    // every call it makes on its owner's behalf is refused, which reads as a
    // broken session rather than a missing token.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;
    drop(first);

    let restarted = service_with_credentials(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "a-fresh-credential",
    );
    restarted.reconcile().await;

    assert_eq!(
        credential_on(&restarted, &created.id).await.as_deref(),
        Some("a-fresh-credential"),
        "a session taken back over must be given the credential its owner has now"
    );
}

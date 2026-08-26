//! What a session showed, and what it presents, across a restart.
//!
//! Split from `recovery_tests` so both stay within the project's file-size
//! limit. The division is the one that matters when something breaks: those
//! cover whether a session comes back at all, and these cover whether it comes
//! back usable — with its history visible and a credential that works.

use std::collections::HashMap;
use std::sync::Arc;

use crate::commit_fault_support::CommitFaultStore;
use crate::event::Event;
use crate::ids::SessionId;
use crate::store::{ContextStore, SessionRecord, SessionStore, StoreError};
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    admin, await_settled, credential_on, service, service_over_with_context, service_with_context,
    service_with_credentials, spec, user, write_screen, FakeAdapter, FakeStore,
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

fn is_context_reference_commit(record: &SessionRecord, events: &[Event]) -> bool {
    events.is_empty() && record.context_ref.is_some()
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
async fn a_live_transcript_wait_does_not_hold_the_session_entry_map() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("live-transcript-lock-owner");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let screen = {
        let entries = svc.entries.read().await;
        entries
            .get(&created.id)
            .expect("the session is live")
            .screen
            .clone()
    };
    let screen_guard = screen.lock().await;
    let transcript = svc.session_transcript(&alice, &created.id);
    tokio::pin!(transcript);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());

    assert!(matches!(
        std::future::Future::poll(transcript.as_mut(), &mut context),
        std::task::Poll::Pending
    ));
    let entries = svc
        .entries
        .try_write()
        .expect("a transcript waiting for the screen releases the session entry map");
    drop(entries);

    drop(screen_guard);
    assert!(transcript
        .await
        .expect("the live transcript is readable")
        .is_empty());
}

#[tokio::test]
async fn a_privileged_reader_cannot_read_a_screen_they_cannot_read_as_a_transcript() {
    // The same bytes by two routes, so they must answer to the same rule. A
    // session's screen shows whatever the agent printed, and the values of the
    // environment bundle it was started with are in its process environment —
    // so an operator who could snapshot a screen could read other people's
    // secrets through a capability documented as listing sessions.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let operator = admin();
    assert!(
        svc.screen_snapshot(&operator, &created.id).await.is_err(),
        "reading every session must not reach into what one is showing"
    );
    assert!(
        svc.session_transcript(&operator, &created.id)
            .await
            .is_err(),
        "the transcript rule this mirrors must still hold"
    );
    assert!(
        svc.screen_snapshot(&alice, &created.id).await.is_ok(),
        "the owner still reads their own screen"
    );
}

#[tokio::test]
async fn listing_every_session_does_not_open_their_event_logs() {
    // Two permissions the provider issues separately. If reading sessions
    // carried reading their histories, the narrower role would silently be
    // the wider one and the grant that exists to gate the log would grant
    // nothing at all.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert!(
        svc.events_since(&admin(), &created.id, None).await.is_err(),
        "a session lister must not read another subject's history"
    );
    assert!(
        svc.events_since(&alice, &created.id, None).await.is_ok(),
        "the owner still reads their own history"
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
async fn a_conflicted_transcript_reference_remains_retryable() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_context_reference_commit,
    ));
    let context = Arc::new(FakeContext::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("transcript-conflict-owner");
    let svc = service_over_with_context(
        store.clone(),
        adapter,
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    write_screen(&svc, &created.id, b"retry this transcript").await;

    store.pause_next();
    let capture = tokio::spawn({
        let svc = Arc::clone(&svc);
        let id = created.id.clone();
        async move { svc.capture_transcript(&id).await }
    });
    store.wait_until_held().await;
    let mut rival = inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("rival reads the record");
    rival.context_ref = Some("transcripts/a-rival-reference".into());
    inner
        .commit_session_idempotent("rival-transcript-reference", &rival, &[])
        .await
        .expect("the rival context reference commits first");
    store.release();

    assert!(
        !capture.await.expect("capture task completes"),
        "a conflicted record reference is not reported as captured"
    );
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("session remains live");
        assert_eq!(entry.captured_len, 0);
        assert!(entry.context_ref.is_none());
    }
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .expect("record remains present")
            .context_ref
            .as_deref(),
        Some("transcripts/a-rival-reference")
    );
    assert_eq!(
        context
            .objects
            .lock()
            .await
            .get(&format!("transcripts/{}", created.id.0))
            .map(Vec::as_slice),
        Some(b"retry this transcript".as_slice()),
        "the stable uploaded object is retained after the record conflict"
    );

    assert!(
        svc.capture_transcript(&created.id).await,
        "unchanged live bookkeeping makes the next sweep retry"
    );
    let durable = inner.get(&created.id).await.unwrap().unwrap();
    assert_eq!(
        durable.context_ref.as_deref(),
        Some(format!("transcripts/{}", created.id.0).as_str())
    );
}

#[tokio::test]
async fn four_transcript_reference_outages_leave_the_capture_retryable() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(
        Arc::clone(&inner),
        is_context_reference_commit,
    ));
    let context = Arc::new(FakeContext::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("transcript-outage-owner");
    let svc = service_over_with_context(
        store.clone(),
        adapter,
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    write_screen(&svc, &created.id, b"persist after recovery").await;

    store.fail_next(4);
    assert!(
        !svc.capture_transcript(&created.id).await,
        "the fourth unavailable response is reported to the capture sweep"
    );
    assert_eq!(store.matched_calls(), 4);
    {
        let entries = svc.entries.read().await;
        let entry = entries.get(&created.id).expect("session remains live");
        assert_eq!(entry.captured_len, 0);
        assert!(entry.context_ref.is_none());
    }
    assert!(inner
        .get(&created.id)
        .await
        .unwrap()
        .expect("record remains present")
        .context_ref
        .is_none());
    assert!(
        context
            .objects
            .lock()
            .await
            .contains_key(&format!("transcripts/{}", created.id.0)),
        "the stable uploaded object is not deleted after a record outage"
    );

    assert!(
        svc.capture_transcript(&created.id).await,
        "the next sweep retries after the store recovers"
    );
    assert_eq!(store.matched_calls(), 5);
    let entries = svc.entries.read().await;
    let entry = entries.get(&created.id).expect("session remains live");
    assert_eq!(entry.captured_len, b"persist after recovery".len());
    assert_eq!(
        entry.context_ref.as_deref(),
        Some(format!("transcripts/{}", created.id.0).as_str())
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
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            Default::default(),
        )
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

#[tokio::test]
async fn a_replaced_session_comes_back_with_what_it_had_shown() {
    // §5: "Everything it has shown remains visible. A session that returns
    // blank has lost the work as far as its owner can tell." Readopting
    // replayed the stored transcript and the provisioning path did not, so a
    // session legitimately given a new runtime came back empty above its
    // prompt even though its screen was in storage.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let alice = user("alice");

    let context = Arc::new(FakeContext::default());
    let first = service_with_context(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    let created = first.create_session(&alice, spec()).await.expect("created");
    await_settled(&first, &alice, &created.id).await;

    // Something on the screen, then stored, exactly as the capture sweep does.
    write_screen(&first, &created.id, b"work the owner can see\r\n").await;
    assert!(
        first.capture_transcript(&created.id).await,
        "the transcript must be captured before it can be restored"
    );

    let handle = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present")
        .runtime_id
        .expect("has a runtime");
    drop(first);
    adapter.lose(&handle).await;

    let restarted = service_with_context(
        Arc::clone(&store),
        Arc::clone(&adapter),
        Arc::clone(&context) as Arc<dyn ContextStore>,
    );
    assert_eq!(restarted.reconcile().await.reprovisioned_sessions, 1);
    await_settled(&restarted, &alice, &created.id).await;

    // Read the way the existing restart test reads it: this screen model
    // records bytes rather than rendering rows, so what it repaints is the
    // observable.
    let transcript = restarted
        .session_transcript(&alice, &created.id)
        .await
        .expect("readable after the runtime was replaced");
    assert_eq!(
        transcript, b"work the owner can see\r\n",
        "a session given a new runtime must still show what it had shown"
    );
}

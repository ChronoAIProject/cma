//! What a session showed, and what it presents, across a restart.
//!
//! Split from `recovery_tests` so both stay within the project's file-size
//! limit. The division is the one that matters when something breaks: those
//! cover whether a session comes back at all, and these cover whether it comes
//! back usable — with its history visible and a credential that works.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ids::SessionId;
use crate::store::{ContextStore, StoreError};
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

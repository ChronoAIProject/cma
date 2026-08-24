//! Who holds a session's terminal, and how they lose it.
//!
//! Exclusivity is what keeps two clients from writing into one another's
//! escape sequences. Takeover is what keeps that exclusivity from locking a
//! person out of their own session when the client holding it has died in a
//! way nothing has noticed yet.

use std::sync::Arc;

use crate::attach::AttachmentClaim;
use crate::error::CoreError;
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, service, service_on_version, spec, user, write_screen, FakeAdapter, FakeStore,
};

/// A service with one running session, and its owner.
async fn attached_session() -> (
    Arc<crate::service::SessionService>,
    crate::authz::Caller,
    crate::ids::SessionId,
) {
    let svc = service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, alice, created.id)
}

#[tokio::test]
async fn a_session_without_a_terminal_is_refused_before_readoption_or_logging() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: layer3_adapter::KernelCapabilities {
            conversation: true,
            ..FakeAdapter::default().capabilities
        },
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), adapter);
    let alice = user("alice");
    let created = svc
        .create_session(
            &alice,
            crate::session::SessionSpec {
                requested_mode: Some(crate::options::SessionMode::Conversation),
                ..spec()
            },
        )
        .await
        .expect("conversation session created");
    let settled = await_settled(&svc, &alice, &created.id).await;
    assert!(!settled.capabilities.terminal);

    // Model a fresh control-plane process: the durable session and its runtime
    // survive, but this process has not adopted an in-memory entry for it.
    svc.entries.write().await.remove(&created.id);
    let events_before = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history readable")
        .len();

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .err(),
        Some(CoreError::CapabilityUnsupported("terminal".into()))
    );
    assert!(
        !svc.entries.read().await.contains_key(&created.id),
        "a refused attachment must not readopt a runtime entry"
    );

    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history readable");
    assert_eq!(
        events.len(),
        events_before,
        "refusing an attachment must not change the durable log"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, crate::event::EventKind::TerminalAttached)),
        "a refused attachment must never be recorded as attached"
    );
}

#[tokio::test]
async fn the_live_grant_is_rechecked_before_the_attachment_is_mutated() {
    let (svc, alice, id) = attached_session().await;
    {
        // This is the state attach can meet when it read the durable terminal
        // grant just before a concurrent plane change replaced the live entry.
        let mut entries = svc.entries.write().await;
        let entry = entries.get_mut(&id).expect("session is live");
        entry.session.capabilities.terminal = false;
        entry.session.capabilities.conversation = true;
    }
    let requested = TerminalGeometry::new(101, 37).expect("valid geometry");

    assert_eq!(
        svc.attach_terminal(&alice, &id, requested, AttachmentClaim::Exclusive)
            .await
            .err(),
        Some(CoreError::CapabilityUnsupported("terminal".into()))
    );

    let entries = svc.entries.read().await;
    let entry = entries.get(&id).expect("session remains live");
    assert_eq!(entry.session.geometry, TerminalGeometry::default());
    assert!(entry.attached.is_none());
    assert_eq!(entry.attachments_issued, 0);
    assert!(
        entry
            .events
            .iter()
            .all(|event| !matches!(event.kind, crate::event::EventKind::TerminalAttached)),
        "the repeated gate must run before attachment state or events change"
    );
}

#[tokio::test]
async fn a_terminal_attachment_carries_its_repaint_and_requested_geometry() {
    let store = Arc::new(FakeStore::default());
    let svc = service_on_version(store, Arc::new(FakeAdapter::default()), "runtime:test");
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    write_screen(&svc, &created.id, b"work already on screen\r\n").await;
    let geometry = TerminalGeometry::new(132, 43).expect("valid geometry");

    let attachment = svc
        .attach_terminal(&alice, &created.id, geometry, AttachmentClaim::Exclusive)
        .await
        .expect("terminal session attaches");

    assert_eq!(attachment.geometry, geometry);
    assert_eq!(attachment.repaint, b"work already on screen\r\n");
}

#[tokio::test]
async fn a_second_client_is_refused_while_one_holds_the_terminal() {
    // Two clients writing into one screen corrupt each other's view in a way
    // neither can see, which is why the attachment is exclusive at all.
    let (svc, alice, id) = attached_session().await;
    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the first client attaches");

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .err(),
        Some(CoreError::AlreadyAttached)
    );
}

#[tokio::test]
async fn a_takeover_supersedes_the_holder_and_tells_them() {
    // The lock-out this exists to prevent: a client whose connection died in a
    // way the server has not noticed still holds the attachment, and its owner
    // cannot get back into their own session.
    let (svc, alice, id) = attached_session().await;
    let mut first = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the first client attaches");

    let _second = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover,
        )
        .await
        .expect("the owner may take their own terminal back");

    assert!(
        first.superseded.try_recv().is_ok(),
        "a superseded client must be told, not merely starved of output"
    );
}

#[tokio::test]
async fn the_superseded_client_leaving_does_not_detach_its_replacement() {
    // The race this is about: the old client drops its handle after the new
    // one is already live. An unscoped release would detach the person who
    // just took their session back, and they would watch it go dead.
    let (svc, alice, id) = attached_session().await;
    let first = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");
    let _second = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover,
        )
        .await
        .expect("takes over");

    drop(first);
    // The release is spawned, so give it a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .err(),
        Some(CoreError::AlreadyAttached),
        "the replacement must still hold the attachment"
    );
}

#[tokio::test]
async fn releasing_the_only_attachment_frees_the_terminal() {
    // The ordinary case: a client goes away and the next one can attach.
    let (svc, alice, id) = attached_session().await;
    let held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");
    drop(held);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        svc.attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive
        )
        .await
        .is_ok(),
        "a released terminal must be attachable again"
    );
}

#[tokio::test]
async fn a_takeover_on_an_unheld_terminal_is_an_ordinary_attach() {
    // Nothing to supersede is not a failure. It is the common case when a
    // client reconnects after its previous connection was already reaped.
    let (svc, alice, id) = attached_session().await;
    assert!(svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn somebody_elses_session_cannot_be_taken_over() {
    // Takeover is a stronger claim than attach, so it must not be a way around
    // the ownership check that guards the weaker one.
    let (svc, _alice, id) = attached_session().await;
    assert_eq!(
        svc.attach_terminal(
            &user("mallory"),
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Takeover
        )
        .await
        .err(),
        Some(CoreError::Forbidden),
        "a session somebody does not own must not be reachable by taking it over"
    );
}

#[tokio::test]
async fn the_client_holding_the_terminal_can_still_type_into_it() {
    // The defect this guards, and it is one this file's own subject invites:
    // a guard against writing while somebody is attached, put in the operation
    // itself, refuses the attachment its own keystrokes. Every character a PTY
    // client sends goes through `send_input` while it holds the terminal.
    let (svc, alice, id) = attached_session().await;
    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the client attaches");

    svc.send_input(&alice, &id, b"ls\n".to_vec())
        .await
        .expect("an attached client types into its own terminal");
    svc.resize_terminal(&alice, &id, TerminalGeometry::new(100, 30).expect("valid"))
        .await
        .expect("and declares the geometry it is drawing for");
    svc.send_signal(&alice, &id, "INT".to_string())
        .await
        .expect("and can stop what it started");
}

#[tokio::test]
async fn whether_the_terminal_is_held_is_a_question_anyone_may_ask_of_their_own() {
    // Asked by an interface about to write on behalf of somebody who is not the
    // holder. A question rather than a refusal, because the answer differs by
    // caller and the operation cannot tell them apart.
    let (svc, alice, id) = attached_session().await;
    assert!(
        !svc.terminal_is_held(&alice, &id)
            .await
            .expect("readable by its owner"),
        "nobody is attached yet"
    );

    let _held = svc
        .attach_terminal(
            &alice,
            &id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("the client attaches");

    assert!(svc.terminal_is_held(&alice, &id).await.expect("readable"));
}

#[tokio::test]
async fn a_stranger_may_not_ask_whether_somebody_elses_terminal_is_held() {
    // It is a fact about someone else's session, and answering reveals that
    // the session exists.
    let (svc, _alice, id) = attached_session().await;
    let mallory = user("mallory");
    assert!(svc.terminal_is_held(&mallory, &id).await.is_err());
}

#[tokio::test]
async fn process_exit_is_recorded_after_final_output_without_changing_lifecycle() {
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::new(FakeStore::default()), Arc::clone(&adapter));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let mut held = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");

    let runtime = adapter.senders.lock().await[0].clone();
    runtime
        .send(layer3_adapter::TerminalOutput::Bytes(
            b"last paint".to_vec(),
        ))
        .await
        .expect("output reaches the core");
    runtime
        .send(layer3_adapter::TerminalOutput::Exited { code: Some(23) })
        .await
        .expect("the ending reaches the core");

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("output arrives in time")
            .expect("the final output is readable"),
        b"last paint"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("the live lane closes in time")
            .is_err(),
        "the live lane closes after its final byte"
    );
    assert_eq!(
        *held.ending.borrow(),
        Some(crate::lease::Detached::ProcessExited { code: Some(23) })
    );
    assert_eq!(
        svc.get_session(&alice, &created.id)
            .await
            .expect("the session remains readable")
            .status,
        crate::session::SessionStatus::Ready,
        "reporting a process ending must not decide session lifecycle"
    );
}

#[tokio::test]
async fn a_terminal_channel_lost_without_a_report_closes_without_fabricating_exit() {
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::new(FakeStore::default()), Arc::clone(&adapter));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    let mut held = svc
        .attach_terminal(
            &alice,
            &created.id,
            TerminalGeometry::default(),
            AttachmentClaim::Exclusive,
        )
        .await
        .expect("attaches");

    adapter.senders.lock().await.clear();

    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), held.output.recv())
            .await
            .expect("the lost lane is noticed")
            .is_err()
    );
    assert_eq!(
        *held.ending.borrow(),
        None,
        "a closed transport is not evidence that the process exited"
    );
}

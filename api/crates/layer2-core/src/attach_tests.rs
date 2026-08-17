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
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

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

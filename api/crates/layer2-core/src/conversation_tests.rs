//! What a session's log holds once its runtime starts talking.
//!
//! The property under all of these is that the log is the record. Whatever a
//! client saw, a client that arrives later — or comes back after a restart —
//! must be able to reconstruct from the sequence alone.

use std::sync::Arc;

use crate::conversation_support::{asking, chunk, conversation, said, until_logged};
use crate::event::EventKind;
use crate::session::{SessionStatus, StopReason};
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn a_tool_call_and_its_progress_are_recorded() {
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(
                    layer3_adapter::ToolCallId::new("call-1"),
                    "read a file".to_string(),
                ),
            )))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ToolCallStarted { call_id, title, .. }
                if call_id == "call-1" && title == "read a file"
        ))
        .await,
        "a tool call the agent made must be in the record"
    );
}

#[tokio::test]
async fn a_message_being_typed_is_liveness_and_never_numbered() {
    // Numbering deltas would punch permanent holes in the sequence a resuming
    // client cursors on, and replaying them would show somebody their
    // conversation being typed a second time.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        for piece in ["Hel", "lo ", "there"] {
            kernel
                .updates
                .send(chunk(piece))
                .await
                .expect("the core is listening");
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::MessageCommitted { .. })),
        "nothing was committed, so nothing may be in the log: {events:?}"
    );
}

#[tokio::test]
async fn usage_is_reported_and_read_back() {
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::UsageUpdate(
                layer3_adapter::UsageUpdate::new(1290, 258_400),
            )))
            .await
            .expect("the core is listening");
    }
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported { context_used, .. } if *context_used == 1290
        ))
        .await,
        "usage must reach the log, which is what get_usage reads"
    );

    let usage = svc.get_usage(&alice, &id).await.expect("usage is readable");
    assert_eq!(usage.context_used, 1290);
}

#[tokio::test]
async fn a_session_without_the_usage_capability_is_refused_by_name() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(
        svc.get_usage(&alice, &created.id).await.err(),
        Some(crate::error::CoreError::CapabilityUnsupported(
            "usage".into()
        )),
        "a terminal session has no tokens to count and must say so"
    );
}

#[tokio::test]
async fn an_approval_parks_the_session_where_a_client_can_see_it() {
    // `RequiresAction` is the one stop reason that is explicitly not terminal.
    // A client shows this as a question; anything else reads as a session that
    // died holding somebody's work.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("the conversation was opened")
            .updates
            .send(layer3_adapter::ConversationUpdate::Asked(asking(
                "req-1",
                "run a command",
            )))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ApprovalRequested { request_id, action, .. }
                if request_id == "req-1" && action.as_deref() == Some("run a command")
        ))
        .await,
        "the request must be in the record, naming what is being asked"
    );

    let session = svc.get_session(&alice, &id).await.expect("readable");
    assert_eq!(session.status, SessionStatus::Idle);
    assert_eq!(session.stop_reason, Some(StopReason::RequiresAction));
    assert!(!session.stop_reason.expect("present").is_terminal());
}

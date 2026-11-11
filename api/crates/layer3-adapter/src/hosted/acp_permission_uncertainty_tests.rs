use std::{
    pin::Pin,
    task::{Context, Poll},
};

use super::*;
use futures_util::{Sink, Stream};
use tokio_tungstenite::tungstenite::Error;

/// Deliver the write but report an error from its completed flush.
pub(super) struct UncertainSocket {
    pub(super) inner: Socket,
    pub(super) fail_flush: Arc<AtomicBool>,
}

impl Stream for UncertainSocket {
    type Item = Result<Message, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Sink<Message> for UncertainSocket {
    type Error = Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }
    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Error> {
        Pin::new(&mut self.inner).start_send(item)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Ready(Ok(())) if self.fail_flush.swap(false, Ordering::SeqCst) => Poll::Ready(
                Err(Error::Io(std::io::Error::other("injected uncertain flush"))),
            ),
            other => other,
        }
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[tokio::test]
async fn delivered_answer_with_failed_flush_keeps_original_decision() {
    let (opened, mut server, fail_flush, _received) = open(true).await;
    let permission = accept(&opened.bridge, 3, 1).await;
    fail_flush.store(true, Ordering::SeqCst);
    assert!(matches!(
        opened
            .bridge
            .bounded_permission_answer(
                &permission.id,
                permission.locator.as_ref(),
                "conversation-1",
                PermissionDecision::Allow,
            )
            .await,
        Err(AdapterError::Unavailable(_))
    ));
    let delivered = receive_json(&mut server).await;
    assert_eq!(delivered["result"]["outcome"]["optionId"], "allow");
    opened
        .bridge
        .bounded_permission_answer(
            &permission.id,
            permission.locator.as_ref(),
            "conversation-1",
            PermissionDecision::Deny,
        )
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await, delivered);
    // Native replay at a new cursor uses the same answer and emits no approval.
    let (capture, mut updates) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained_bytes(4, &super::permission(1, "allow")), &capture)
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await, delivered);
    assert!(matches!(
        updates.try_recv().unwrap(),
        ConversationUpdate::RuntimeOutput { update: None, .. }
    ));
    stop(opened).await;
}

#[tokio::test]
async fn cancelled_send_keeps_selected_answer_for_retry() {
    let (opened, mut server, _, _received) = open(true).await;
    let permission = accept(&opened.bridge, 3, 1).await;
    let lock = opened.bridge.sink.lock().await;
    let mut write = Box::pin(opened.bridge.bounded_permission_answer(
        &permission.id,
        permission.locator.as_ref(),
        "conversation-1",
        PermissionDecision::Allow,
    ));
    assert!(futures_util::poll!(&mut write).is_pending());
    let entry = opened
        .bridge
        .permission_state
        .as_ref()
        .unwrap()
        .lock()
        .await
        .get(&permission.id)
        .unwrap();
    assert_eq!(
        entry.answer().unwrap()["result"]["outcome"]["optionId"],
        "allow"
    );
    drop(write);
    drop(lock);
    opened
        .bridge
        .bounded_permission_answer(
            &permission.id,
            permission.locator.as_ref(),
            "conversation-1",
            PermissionDecision::Abort,
        )
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["result"]["outcome"]["optionId"],
        "allow"
    );
    stop(opened).await;
}

#[tokio::test]
async fn durable_locator_restores_bounded_state_and_wrong_session_is_refused() {
    let (opened, mut server, _, _received) = open(true).await;
    let locator = crate::hosted::acp_inbound::permission_locator(
        "conversation-1",
        &serde_json::json!(41),
        &[PermissionOption::new(
            "allow",
            "Allow",
            PermissionOptionKind::AllowOnce,
        )],
    );
    let id = crate::hosted::acp_inbound::permission_id(&locator);
    assert!(matches!(
        opened
            .bridge
            .bounded_permission_answer(
                &id,
                Some(&locator),
                "wrong-session",
                PermissionDecision::Deny,
            )
            .await,
        Err(AdapterError::Rejected(_))
    ));
    assert!(opened
        .bridge
        .permission_state
        .as_ref()
        .unwrap()
        .lock()
        .await
        .get(&id)
        .is_none());
    opened
        .bridge
        .bounded_permission_answer(
            &id,
            Some(&locator),
            "conversation-1",
            PermissionDecision::Allow,
        )
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["result"]["outcome"]["optionId"],
        "allow"
    );
    assert!(opened
        .bridge
        .orphaned_prompt_response
        .load(Ordering::SeqCst));
    stop(opened).await;
}

#[tokio::test]
async fn blocked_sink_cannot_hold_permission_refusal_past_its_deadline() {
    let (opened, _server, _, _received) = open(true).await;
    let sink = opened.bridge.sink.lock().await;
    let (capture, mut updates) = UpdateRelay::capture();
    let bytes = retained_bytes(3, &permission(1, &"x".repeat(MAX_OPTION_BYTES + 1)));
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        opened.bridge.receive_output(&bytes, &capture),
    )
    .await;
    assert!(matches!(result, Ok(Err(AdapterError::Rejected(_)))));
    // Tokio rounds deadlines up to its next one-millisecond timer tick.
    assert!((Duration::from_secs(1)..=Duration::from_millis(1001)).contains(&started.elapsed()));
    assert!(
        updates.try_recv().is_err(),
        "notification timeout publishes no receipt or effect"
    );
    drop(sink);
    tokio::time::resume();
    stop(opened).await;
}

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Notify, Semaphore};

pub(super) struct Guard {
    allowed: AtomicBool,
    pub(super) entered: Notify,
    release: Semaphore,
}

impl Guard {
    pub(super) fn new(allowed: bool, permits: usize) -> Arc<Self> {
        Arc::new(Self {
            allowed: AtomicBool::new(allowed),
            entered: Notify::new(),
            release: Semaphore::new(permits),
        })
    }
}

#[async_trait::async_trait]
impl ConversationInputGuard for Guard {
    async fn check(&self) -> crate::Result<()> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.allowed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(AdapterError::Rejected("execution authority expired".into()))
        }
    }
}

fn counting_sender(writes: &Arc<AtomicUsize>) -> ConversationInputSender {
    let writes = writes.clone();
    ConversationInputSender::spawn(4, move |_| {
        writes.fetch_add(1, Ordering::AcqRel);
        async { Ok(()) }
    })
}

#[tokio::test]
async fn guard_refusal_never_invokes_handler_and_preserves_conversation() {
    let writes = Arc::new(AtomicUsize::new(0));
    let sender = counting_sender(&writes);
    assert!(matches!(
        sender
            .send_guarded(ConversationInput::Cancel, Guard::new(false, 1))
            .await,
        ConversationSendOutcome::PreWriteFailure(AdapterError::Rejected(_))
    ));
    assert_eq!(writes.load(Ordering::Acquire), 0);
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(writes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn queued_input_checks_authority_after_the_previous_writer_finishes() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let sender = ConversationInputSender::spawn(2, {
        let entered = entered.clone();
        let release = release.clone();
        let writes = writes.clone();
        move |_| {
            writes.fetch_add(1, Ordering::AcqRel);
            let entered = entered.clone();
            let release = release.clone();
            async move {
                entered.notify_one();
                release.acquire().await.unwrap().forget();
                Ok(())
            }
        }
    });
    let first = tokio::spawn({
        let sender = sender.clone();
        async move { sender.send(ConversationInput::Cancel).await }
    });
    entered.notified().await;
    let guard = Guard::new(true, 1);
    let second = sender.send_guarded(ConversationInput::Cancel, guard.clone());
    tokio::pin!(second);
    tokio::select! {
        biased;
        _ = &mut second => panic!("queued send completed before the writer"),
        _ = tokio::task::yield_now() => {}
    }
    assert_eq!(sender.inner.capacity(), 1);
    guard.allowed.store(false, Ordering::Release);
    release.add_permits(1);
    first.await.unwrap().unwrap();
    assert!(matches!(
        second.await,
        ConversationSendOutcome::PreWriteFailure(_)
    ));
    assert_eq!(writes.load(Ordering::Acquire), 1);
    release.add_permits(1);
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(writes.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn timeout_during_guard_cancels_only_that_request_and_unblocks_writer() {
    let writes = Arc::new(AtomicUsize::new(0));
    let sender = counting_sender(&writes);
    let guard = Guard::new(true, 0);
    assert!(matches!(
        sender
            .send_with_deadline(
                ConversationInput::Cancel,
                Some(guard.clone()),
                Duration::from_millis(20)
            )
            .await,
        ConversationSendOutcome::PreWriteFailure(_)
    ));
    // No release of the guard: its future must be cancelled when its recipient
    // disappears, allowing the same ordered writer to process the next input.
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(writes.load(Ordering::Acquire), 1);
    guard.release.add_permits(1);
    tokio::task::yield_now().await;
    assert_eq!(writes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn dropping_send_during_guard_keeps_writer_and_prevents_late_delivery() {
    let writes = Arc::new(AtomicUsize::new(0));
    let sender = counting_sender(&writes);
    let guard = Guard::new(true, 0);
    let send = tokio::spawn({
        let sender = sender.clone();
        let guard = guard.clone();
        async move { sender.send_guarded(ConversationInput::Cancel, guard).await }
    });
    guard.entered.notified().await;
    send.abort();
    assert!(send.await.unwrap_err().is_cancelled());
    guard.release.add_permits(1);
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(writes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn timeout_after_guard_and_handler_invocation_remains_ambiguous() {
    let writes = Arc::new(AtomicUsize::new(0));
    let sender = ConversationInputSender::spawn(1, {
        let writes = writes.clone();
        move |_| {
            writes.fetch_add(1, Ordering::AcqRel);
            std::future::pending::<crate::Result<()>>()
        }
    });
    assert!(matches!(
        sender
            .send_with_deadline(
                ConversationInput::Cancel,
                Some(Guard::new(true, 1)),
                Duration::from_millis(20)
            )
            .await,
        ConversationSendOutcome::Ambiguous(_)
    ));
    assert_eq!(writes.load(Ordering::Acquire), 1);
    tokio::task::yield_now().await;
    assert!(matches!(
        sender.send_classified(ConversationInput::Cancel).await,
        ConversationSendOutcome::PreWriteFailure(_)
    ));
    assert_eq!(writes.load(Ordering::Acquire), 1);
}

use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Notify, Semaphore};

struct Progress(
    watch::Receiver<u64>,
    watch::Receiver<Option<tokio::time::Instant>>,
);

#[async_trait::async_trait]
impl ConversationSettingsSupport for Progress {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn retained_history_deadline(
        &self,
    ) -> Option<watch::Receiver<Option<tokio::time::Instant>>> {
        Some(self.1.clone())
    }

    async fn managed_context_progress(&self) -> Option<watch::Receiver<u64>> {
        Some(self.0.clone())
    }
}

fn refresh() -> ConversationInput {
    ConversationInput::RefreshContext {
        service_origin: Some("https://cma.nyxid.ai".into()),
        operation_sequence: 8683,
        readiness_receipt: "admitted".into(),
        guidance: "managed".into(),
        previous_guidance: Vec::new(),
        model: "gpt-6".into(),
        reasoning_effort: crate::ReasoningEffort::High,
        permission: crate::PermissionLevel::Autonomous,
        working_directory: None,
    }
}

struct Fixture {
    sender: ConversationInputSender,
    progress: watch::Sender<u64>,
    history: watch::Sender<Option<tokio::time::Instant>>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    writes: Arc<AtomicUsize>,
}

fn fixture() -> Fixture {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let (progress, received) = watch::channel(0);
    let (history, loading) = watch::channel(None);
    let sender = ConversationInputSender::spawn(2, {
        let entered = entered.clone();
        let release = release.clone();
        let writes = writes.clone();
        move |_| {
            let entered = entered.clone();
            let release = release.clone();
            let writes = writes.clone();
            async move {
                writes.fetch_add(1, Ordering::AcqRel);
                entered.notify_one();
                release.acquire().await.unwrap().forget();
                Ok(())
            }
        }
    })
    .with_settings_support(Arc::new(Progress(received, loading)));
    Fixture {
        sender,
        progress,
        history,
        entered,
        release,
        writes,
    }
}

async fn advance(seconds: u64) {
    tokio::time::advance(Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn context_delivery_waits_for_durable_replay_beyond_fifteen_seconds() {
    for guarded in [false, true] {
        let f = fixture();
        let send = tokio::spawn({
            let sender = f.sender.clone();
            async move {
                if guarded {
                    sender
                        .send_guarded(refresh(), guard_tests::Guard::new(true, 1))
                        .await
                } else {
                    sender.send_classified(refresh()).await
                }
            }
        });
        f.entered.notified().await;
        for cursor in 1..=4 {
            advance(10).await;
            assert!(
                !send.is_finished(),
                "durable replay must keep the original refresh alive"
            );
            f.progress.send(cursor).unwrap();
            tokio::task::yield_now().await;
        }
        f.release.add_permits(1);
        assert!(matches!(
            send.await.unwrap(),
            ConversationSendOutcome::WriteConfirmed
        ));
        assert_eq!(f.writes.load(Ordering::Acquire), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_duplicate_or_regressing_progress_remains_ambiguous() {
    for next in [None, Some(10), Some(9)] {
        let f = fixture();
        f.progress.send(10).unwrap();
        let send = tokio::spawn({
            let sender = f.sender.clone();
            async move { sender.send_classified(refresh()).await }
        });
        f.entered.notified().await;
        advance(10).await;
        if let Some(next) = next {
            f.progress.send(next).unwrap();
            tokio::task::yield_now().await;
        }
        advance(6).await;
        assert!(matches!(
            send.await.unwrap(),
            ConversationSendOutcome::Ambiguous(_)
        ));
        f.release.add_permits(1);
        tokio::task::yield_now().await;
        assert!(f.sender.inner.is_closed());
        assert_eq!(f.writes.load(Ordering::Acquire), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn continuous_progress_cannot_exceed_absolute_context_limit() {
    let f = fixture();
    let send = tokio::spawn({
        let sender = f.sender.clone();
        async move { sender.send_classified(refresh()).await }
    });
    f.entered.notified().await;
    for cursor in 1..=11 {
        advance(10).await;
        assert!(!send.is_finished());
        f.progress.send(cursor).unwrap();
        tokio::task::yield_now().await;
    }
    advance(11).await;
    assert!(matches!(
        send.await.unwrap(),
        ConversationSendOutcome::Ambiguous(_)
    ));
    assert_eq!(f.writes.load(Ordering::Acquire), 1);
}

#[tokio::test(start_paused = true)]
async fn progress_does_not_extend_a_pending_authority_guard_or_other_input() {
    let f = fixture();
    let guard = guard_tests::Guard::new(true, 0);
    let send = tokio::spawn({
        let sender = f.sender.clone();
        let guard = guard.clone();
        async move { sender.send_guarded(refresh(), guard).await }
    });
    guard.entered.notified().await;
    f.history
        .send(Some(tokio::time::Instant::now() + Duration::from_secs(960)))
        .unwrap();
    advance(10).await;
    f.progress.send(1).unwrap();
    tokio::task::yield_now().await;
    advance(6).await;
    assert!(matches!(
        send.await.unwrap(),
        ConversationSendOutcome::PreWriteFailure(_)
    ));
    assert_eq!(f.writes.load(Ordering::Acquire), 0);
    f.release.add_permits(1);
    f.sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(f.writes.load(Ordering::Acquire), 1);

    let send = tokio::spawn({
        let sender = f.sender.clone();
        async move { sender.send_classified(ConversationInput::Cancel).await }
    });
    f.entered.notified().await;
    advance(1).await;
    f.progress.send(2).unwrap();
    tokio::task::yield_now().await;
    advance(2).await;
    assert!(matches!(
        send.await.unwrap(),
        ConversationSendOutcome::Ambiguous(_)
    ));
}

fn settings_or_refresh(configure: bool) -> ConversationInput {
    if configure {
        ConversationInput::Configure {
            model: "gpt-6".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Autonomous,
            system_prompt: Some("instructions".into()),
        }
    } else {
        refresh()
    }
}

async fn start_history(
    f: &Fixture,
    configure: bool,
) -> tokio::task::JoinHandle<ConversationSendOutcome> {
    let sender = f.sender.clone();
    let send =
        tokio::spawn(async move { sender.send_classified(settings_or_refresh(configure)).await });
    f.entered.notified().await;
    f.history
        .send(Some(tokio::time::Instant::now() + Duration::from_secs(960)))
        .unwrap();
    tokio::task::yield_now().await;
    send
}

#[tokio::test(start_paused = true)]
async fn history_delivery_restores_the_ordinary_deadline_after_loading() {
    for configure in [false, true] {
        let f = fixture();
        let send = start_history(&f, configure).await;
        advance(299).await;
        assert!(!send.is_finished());
        f.history.send(None).unwrap();
        tokio::task::yield_now().await;
        advance(14).await;
        assert!(!send.is_finished());
        advance(2).await;
        assert!(matches!(
            send.await.unwrap(),
            ConversationSendOutcome::Ambiguous(_)
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn history_delivery_ack_progress_cannot_exceed_the_load_absolute_deadline() {
    for configure in [false, true] {
        let f = fixture();
        let send = start_history(&f, configure).await;
        for cursor in 1..=3 {
            advance(240).await;
            assert!(!send.is_finished());
            f.progress.send(cursor).unwrap();
            tokio::task::yield_now().await;
        }
        advance(241).await;
        assert!(matches!(
            send.await.unwrap(),
            ConversationSendOutcome::Ambiguous(_)
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn history_delivery_stalls_after_five_minutes_without_new_ack_progress() {
    for configure in [false, true] {
        for next in [None, Some(10), Some(9)] {
            let f = fixture();
            f.progress.send(10).unwrap();
            let send = start_history(&f, configure).await;
            advance(299).await;
            assert!(!send.is_finished());
            if let Some(next) = next {
                f.progress.send(next).unwrap();
                tokio::task::yield_now().await;
            }
            advance(2).await;
            assert!(matches!(
                send.await.unwrap(),
                ConversationSendOutcome::Ambiguous(_)
            ));
        }
    }
}

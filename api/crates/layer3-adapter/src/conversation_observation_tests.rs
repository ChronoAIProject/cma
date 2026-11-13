use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Notify, Semaphore};

struct Observation {
    entered: Notify,
    release: Semaphore,
    block_progress: bool,
}

impl Observation {
    async fn wait(&self) {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
    }
}

#[async_trait::async_trait]
impl ConversationSettingsSupport for Observation {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_progress(&self) -> Option<watch::Receiver<u64>> {
        if self.block_progress {
            self.wait().await;
        }
        None
    }

    async fn retained_history_deadline(
        &self,
    ) -> Option<watch::Receiver<Option<tokio::time::Instant>>> {
        if !self.block_progress {
            self.wait().await;
        }
        None
    }
}

fn configure() -> ConversationInput {
    ConversationInput::Configure {
        model: "gpt-6".into(),
        reasoning_effort: crate::ReasoningEffort::High,
        permission: crate::PermissionLevel::Autonomous,
        system_prompt: None,
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_settings_observation_expires_before_enqueue_and_preserves_writer() {
    for block_progress in [false, true] {
        for guarded in [false, true] {
            let observation = Arc::new(Observation {
                entered: Notify::new(),
                release: Semaphore::new(0),
                block_progress,
            });
            let writes = Arc::new(AtomicUsize::new(0));
            let sender = ConversationInputSender::spawn(1, {
                let writes = writes.clone();
                move |_| {
                    writes.fetch_add(1, Ordering::AcqRel);
                    async { Ok(()) }
                }
            })
            .with_settings_support(observation.clone());
            let send = tokio::spawn({
                let sender = sender.clone();
                async move {
                    if guarded {
                        sender
                            .send_guarded(configure(), guard_tests::Guard::new(true, 1))
                            .await
                    } else {
                        sender.send_classified(configure()).await
                    }
                }
            });
            observation.entered.notified().await;
            tokio::time::advance(Duration::from_secs(16)).await;
            tokio::task::yield_now().await;
            assert!(
                send.is_finished(),
                "observation must consume the delivery budget"
            );
            assert!(matches!(
                send.await.unwrap(),
                ConversationSendOutcome::PreWriteFailure(_)
            ));
            assert_eq!(writes.load(Ordering::Acquire), 0);
            observation.release.add_permits(1);
            sender.send(ConversationInput::Cancel).await.unwrap();
            assert_eq!(writes.load(Ordering::Acquire), 1);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn settings_observation_time_is_not_added_to_the_delivery_budget() {
    let observation = Arc::new(Observation {
        entered: Notify::new(),
        release: Semaphore::new(0),
        block_progress: true,
    });
    let entered = Arc::new(Notify::new());
    let sender = ConversationInputSender::spawn(1, {
        let entered = entered.clone();
        move |_| {
            entered.notify_one();
            std::future::pending::<crate::Result<()>>()
        }
    })
    .with_settings_support(observation.clone());
    let send = tokio::spawn(async move { sender.send_classified(configure()).await });
    observation.entered.notified().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    observation.release.add_permits(1);
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert!(!send.is_finished());
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(
        send.is_finished(),
        "observation must not restart the delivery budget"
    );
    assert!(matches!(
        send.await.unwrap(),
        ConversationSendOutcome::Ambiguous(_)
    ));
}

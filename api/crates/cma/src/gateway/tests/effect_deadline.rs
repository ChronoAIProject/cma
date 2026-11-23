use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn expired_effect_deadline_refuses_before_polling_adapter_or_authentication() {
    let mut bot = bot().await;
    let work = bot.work.as_mut().unwrap();
    work.effect_protocol = 1;
    work.holder = Some("expired-worker".into());
    work.effect = Some(layer2_core::bots::BotEffect {
        id: uuid::Uuid::new_v4().to_string(),
        step: work.step.clone(),
        deadline: layer2_core::rfc3339_at(std::time::SystemTime::UNIX_EPOCH),
    });
    let polled = AtomicBool::new(false);
    assert_eq!(
        bounded_effect(&bot, async {
            polled.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await,
        Err(BotError::Conflict)
    );
    assert!(!polled.load(Ordering::SeqCst));
    bot.work.as_mut().unwrap().effect = None;
    assert_eq!(
        bounded_effect(&bot, async {
            polled.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await,
        Err(BotError::Conflict)
    );
    assert!(!polled.load(Ordering::SeqCst));
}

pub(super) async fn reserved_bot() -> Bot {
    let mut bot = bot().await;
    let work = bot.work.as_mut().unwrap();
    work.effect_protocol = 1;
    work.holder = Some("effect-worker".into());
    let deadline = layer2_core::rfc3339_at(std::time::SystemTime::now() + Duration::from_secs(120));
    work.expires_at = Some(deadline.clone());
    work.effect = Some(BotEffect {
        id: uuid::Uuid::new_v4().to_string(),
        step: work.step.clone(),
        deadline,
    });
    bot
}

#[tokio::test(start_paused = true)]
async fn resumed_authentication_cannot_admit_a_mutation_after_deadline() {
    struct PausedTokens {
        ready: AtomicBool,
        resumed: AtomicBool,
    }
    #[async_trait::async_trait]
    impl OwnerTokens for PausedTokens {
        async fn bot_owner_token(
            &self,
            _: &SubjectId,
        ) -> Result<crate::auth::persistence::SecretValue, BotError> {
            std::future::poll_fn(|_| {
                if self.ready.load(Ordering::SeqCst) {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            })
            .await;
            self.resumed.store(true, Ordering::SeqCst);
            Ok(crate::auth::persistence::SecretValue::new(
                uuid::Uuid::new_v4().to_string(),
            ))
        }
    }
    let server = Server::start().await;
    let auth = Arc::new(PausedTokens {
        ready: AtomicBool::new(false),
        resumed: AtomicBool::new(false),
    });
    let mut external = super::lifecycle::external_at(&server, &server.base, &server.base);
    Arc::get_mut(&mut external).unwrap().auth = auth.clone();
    let bot = reserved_bot().await;
    let store = InMemoryBotStore::default();
    store.insert(&bot).await.unwrap();
    let future = external.mint(&bot, BotKeyRole::Route);
    tokio::pin!(future);
    assert!(futures_util::poll!(future.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(121)).await;
    auth.ready.store(true, Ordering::SeqCst);
    // Tokio polls the now-ready composite future before its expired timeout.
    // The per-send fence, rather than cancellation, must reject admission.
    assert!(matches!(future.await, Err(BotError::Conflict)));
    assert!(auth.resumed.load(Ordering::SeqCst));
    assert!(server.identity.lock().unwrap().calls.is_empty());
    assert_eq!(
        store.get(&bot.bot_id).await.unwrap().unwrap().work,
        bot.work
    );
}

#[tokio::test]
async fn admission_rejects_replaced_reservation_identity() {
    let mut bot = reserved_bot().await;
    let admission = EffectAdmission::from_bot(&bot).unwrap().unwrap();
    assert_eq!(admission.check(&bot), Ok(()));
    bot.work.as_mut().unwrap().effect.as_mut().unwrap().id = uuid::Uuid::new_v4().to_string();
    assert_eq!(admission.check(&bot), Err(BotError::Conflict));
}

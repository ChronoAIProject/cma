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

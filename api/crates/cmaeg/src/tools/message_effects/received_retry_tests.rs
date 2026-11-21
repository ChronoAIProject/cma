use super::{received_fixture::*, *};
use crate::channels::definition::fixture as definition_fixture;
use cmaeg_core::{channel::Secret, seal::EventRef};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tokio::sync::Semaphore;

#[tokio::test]
async fn received_reaction_uncertain_and_lost_receipt_fence_restart_remint_and_new_keys() {
    for native in [Native::Telegram, Native::Discord] {
        for lost_receipt in [false, true] {
            let mut f = Fixture::new(native, false, false).await;
            let reference = f.reference().await;
            match native {
                Native::Telegram => {
                    f.telegram.uncertain.store(!lost_receipt, Ordering::SeqCst);
                    f.telegram
                        .fail_receipt
                        .store(lost_receipt, Ordering::SeqCst);
                }
                Native::Discord => {
                    let mut memory = f.discord.memory.lock().unwrap();
                    if lost_receipt {
                        memory.fail_receipt = Some(f.app.backend.fail_put.clone());
                    } else {
                        memory.reject_write = Some((1, StatusCode::INTERNAL_SERVER_ERROR));
                    }
                }
            }
            let first = f
                .app
                .state
                .set_message_reaction(f.app.caller(), &reference, "uncertain", request("👍"))
                .await
                .unwrap();
            assert_eq!(first.delivery.kind(), "unknown");
            f.telegram.uncertain.store(false, Ordering::SeqCst);
            f.telegram.fail_receipt.store(false, Ordering::SeqCst);
            f.discord.memory.lock().unwrap().reject_write = None;
            f.discord.memory.lock().unwrap().fail_receipt = None;
            f.app.backend.fail_put.store(false, Ordering::SeqCst);
            f.app.restart();
            let reminted = f.reference().await;
            assert_ne!(reference, reminted);
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reminted, "uncertain", request("👍"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reminted, "another", request("❤️"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
            assert_eq!(f.writes(), 1);
            let a = reference
                .open(&f.app.state.keys)
                .unwrap()
                .payload
                .received_reaction
                .unwrap();
            let full_expiry = a.received_at + cmaeg_core::seal::received_reaction::MAX_AGE;
            assert!(a.expires_at < full_expiry);
            let retained = f
                .app
                .backend
                .records()
                .into_iter()
                .filter_map(|(id, row)| {
                    let opened =
                        cmaeg_core::seal::open_interaction(&f.app.state.keys, &id, &row.blob)
                            .ok()?;
                    serde_json::from_slice::<Value>(&opened.payload).ok()
                })
                .collect::<Vec<_>>();
            let fence = match native {
                Native::Telegram => "telegram_received_mutation_v1",
                Native::Discord => "discord_message_mutation_v1",
            };
            for kind in ["received_reaction_v1", fence] {
                let records = retained
                    .iter()
                    .filter(|row| row["kind"] == kind)
                    .collect::<Vec<_>>();
                assert!(!records.is_empty());
                assert!(records.iter().all(|row| row["expires_at"] == full_expiry));
            }
        }
    }
}

#[tokio::test]
async fn received_reaction_rotation_remint_replays_stable_receipt_without_resend() {
    for native in [Native::Telegram, Native::Discord] {
        let mut f = Fixture::new(native, false, false).await;
        let original = f.reference().await;
        let first = f
            .app
            .state
            .set_message_reaction(f.app.caller(), &original, "stable", request("👍"))
            .await
            .unwrap();
        assert_eq!(first.delivery.kind(), "confirmed");
        definition_fixture::replace_default_credential(
            &mut f.app.definition,
            Some("new-provider-credential"),
        );
        match native {
            Native::Telegram => {
                f.app.definition.secrets.telegram_bot_token =
                    Some(Secret::new("77:rotated-received-fixture"))
            }
            Native::Discord => {
                f.app.definition.secrets.discord_bot_token =
                    Some(Secret::new("rotated-discord-fixture"))
            }
        }
        f.app.replace();
        f.app.restart();
        assert!(f
            .app
            .state
            .set_message_reaction(f.app.caller(), &original, "stale", request("👍"))
            .await
            .is_err());
        let renewed = f.reference().await;
        assert_eq!(
            f.app
                .state
                .set_message_reaction(f.app.caller(), &renewed, "stable", request("👍"))
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            f.app
                .state
                .set_message_reaction(f.app.caller(), &renewed, "stable", request("❤️"))
                .await
                .unwrap_err()
                .code,
            ProblemCode::Conflict
        );
        assert_eq!(f.writes(), 1);
    }
}

#[tokio::test]
async fn received_reaction_same_target_concurrent_consumers_serialize_different_keys() {
    for native in [Native::Telegram, Native::Discord] {
        let f = Fixture::new(native, true, true).await;
        let named = f.reference().await;
        let channel = f
            .app
            .state
            .resolver
            .resolve_current(f.app.event.channel_id.as_str())
            .await
            .unwrap();
        let default_caller = AgentCaller {
            key_hash: *channel.consumer_key_hash(None).unwrap(),
        };
        let event = EventRef::seal(f.default_event.as_ref().unwrap(), &f.app.state.keys).unwrap();
        let default = f
            .app
            .state
            .verify_context(default_caller, &event, f.app.activity.clone())
            .await
            .unwrap()
            .received_message
            .unwrap()
            .message_ref;
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        f.telegram.delay_ms.store(200, Ordering::SeqCst);
        if native == Native::Discord {
            f.discord.memory.lock().unwrap().message_gate =
                Some((entered.clone(), release.clone()));
        }
        let first =
            f.app
                .state
                .set_message_reaction(f.app.caller(), &named, "named", request("👍"));
        let second = async {
            if native == Native::Discord {
                entered.acquire().await.unwrap().forget();
            }
            let outcome = f
                .app
                .state
                .set_message_reaction(default_caller, &default, "default", request("❤️"))
                .await;
            release.add_permits(1);
            outcome
        };
        let (first, second) = tokio::join!(first, second);
        let outcomes = [
            first.unwrap().delivery.kind(),
            second.unwrap().delivery.kind(),
        ];
        assert!(outcomes.contains(&"confirmed") && outcomes.contains(&"unknown"));
        assert_eq!(f.writes(), 1);
    }
}

#[tokio::test]
async fn received_discord_reaction_custody_rotation_after_metadata_prevents_all_writes() {
    let mut f = Fixture::new(Native::Discord, false, true).await;
    let reference = f.reference().await;
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    f.discord.memory.lock().unwrap().message_gate = Some((entered.clone(), release.clone()));
    // Prepare the new encrypted revision before borrowing state for the in-flight call.
    f.app.definition.secrets.discord_bot_token = Some(Secret::new("rotated-after-read"));
    let first =
        f.app
            .state
            .set_message_reaction(f.app.caller(), &reference, "rotation", request("👍"));
    let rotate = async {
        entered.acquire().await.unwrap().forget();
        f.app.replace();
        release.add_permits(1);
    };
    let (first, ()) = tokio::join!(first, rotate);
    assert_eq!(first.unwrap().delivery.kind(), "rejected");
    assert_eq!(f.writes(), 0);
}

#[tokio::test]
async fn received_discord_reaction_partial_remove_add_keeps_target_fence() {
    let mut f = Fixture::new(Native::Discord, false, false).await;
    let reference = f.reference().await;
    {
        let mut memory = f.discord.memory.lock().unwrap();
        memory.message["reactions"] = json!([{"me":true,"emoji":{"id":null,"name":"👍"}}]);
        memory.reject_write = Some((2, StatusCode::TOO_MANY_REQUESTS));
    }
    assert_eq!(
        f.app
            .state
            .set_message_reaction(f.app.caller(), &reference, "partial", request("❤️"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    f.app.restart();
    assert_eq!(
        f.app
            .state
            .set_message_reaction(f.app.caller(), &reference, "new", request("❤️"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(f.writes(), 2);
}

#[tokio::test]
async fn received_reaction_rate_limit_is_durable_and_not_automatically_retried() {
    for native in [Native::Telegram, Native::Discord] {
        let f = Fixture::new(native, false, false).await;
        let reference = f.reference().await;
        *f.telegram.status.lock().unwrap() = StatusCode::TOO_MANY_REQUESTS;
        f.discord.memory.lock().unwrap().reject_write = Some((1, StatusCode::TOO_MANY_REQUESTS));
        let first = f
            .app
            .state
            .set_message_reaction(f.app.caller(), &reference, "limited", request("👍"))
            .await
            .unwrap();
        assert_eq!(
            first.delivery,
            DeliveryOutcome::RateLimited { retry_after: 2 }
        );
        assert_eq!(
            f.app
                .state
                .set_message_reaction(f.app.caller(), &reference, "limited", request("👍"))
                .await
                .unwrap(),
            first
        );
        assert_eq!(f.writes(), 1);
    }
}

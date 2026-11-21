use super::{received_fixture::*, *};
use cmaeg_core::{message_effect::TelegramReaction, seal::EventRef};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn received_reaction_exact_original_set_replay_replace_clear_and_edit_delete_refusal() {
    for native in [Native::Telegram, Native::Discord] {
        for threaded in [false, true] {
            let f = Fixture::new(native, false, threaded).await;
            let reference = f.reference().await;
            let payload = reference.open(&f.app.state.keys).unwrap().payload;
            let authority = payload.received_reaction.as_ref().unwrap();
            assert_eq!(payload.platform_message_id, "800");
            assert_eq!(authority.actor_id, "42");
            assert_eq!(authority.thread_id.is_some(), threaded);
            assert_eq!(authority.event_sha256, receipts::hash(&f.app.event));
            let caps = f
                .app
                .state
                .message_effect_capabilities(f.app.caller(), &reference)
                .await
                .unwrap();
            assert_eq!(caps.max_reactions, 1);
            assert!(!caps.delete && caps.reaction);
            let first = f
                .app
                .state
                .set_message_reaction(f.app.caller(), &reference, "first", request("👍"))
                .await
                .unwrap();
            assert_eq!(first.delivery.kind(), "confirmed");
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reference, "first", request("👍"))
                    .await
                    .unwrap(),
                first
            );
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reference, "first", request("❤️"))
                    .await
                    .unwrap_err()
                    .code,
                ProblemCode::Conflict
            );
            if native == Native::Discord {
                f.discord.memory.lock().unwrap().message["reactions"] = json!([
                    {"me":true,"emoji":{"id":null,"name":"👍"}},
                    {"me":false,"emoji":{"id":null,"name":"👀"}},
                    {"me":false,"me_burst":true,"emoji":{"id":null,"name":"🔥"}}
                ]);
            }
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reference, "replace", request("❤️"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            if native == Native::Discord {
                f.discord.memory.lock().unwrap().message["reactions"] = json!([
                    {"me":true,"emoji":{"id":null,"name":"❤️"}},
                    {"me":false,"emoji":{"id":null,"name":"👀"}}
                ]);
            }
            assert_eq!(
                f.app
                    .state
                    .set_message_reaction(f.app.caller(), &reference, "clear", request(""))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "confirmed"
            );
            let before = f.writes();
            assert_eq!(
                f.app
                    .state
                    .delete_message(f.app.caller(), &reference, "delete")
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unsupported"
            );
            assert_eq!(
                f.app
                    .state
                    .update(
                        f.app.caller(),
                        &reference,
                        serde_json::from_value(json!({"text":"edited"})).unwrap()
                    )
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unsupported"
            );
            assert_eq!(f.writes(), before);
            match native {
                Native::Telegram => {
                    let calls = f.telegram.calls.lock().unwrap();
                    assert_eq!(calls.len(), 3);
                    for call in calls.iter() {
                        assert_eq!(call["chat_id"], if threaded { "-100" } else { "42" });
                        assert_eq!(call["message_id"], "800");
                        assert!(call.get("message_thread_id").is_none());
                    }
                    assert_eq!(calls[0]["reaction"], json!([{"type":"emoji","emoji":"👍"}]));
                    assert_eq!(calls[1]["reaction"], json!([{"type":"emoji","emoji":"❤️"}]));
                    assert_eq!(calls[2]["reaction"], json!([]));
                }
                Native::Discord => {
                    let writes = &f.discord.memory.lock().unwrap().writes;
                    assert_eq!(writes.len(), 4);
                    assert_eq!(
                        writes.iter().map(|w| w.0.clone()).collect::<Vec<_>>(),
                        vec![Method::PUT, Method::DELETE, Method::PUT, Method::DELETE]
                    );
                    assert!(writes.iter().all(|(_, path)| path
                        .starts_with("/api/v10/channels/99/messages/800/reactions/")
                        && path.ends_with("/@me")));
                    assert!(writes.iter().all(|(_, path)| !path.contains("%F0%9F%91%80")
                        && !path.contains("%F0%9F%94%A5")));
                }
            }
        }
    }
}

#[tokio::test]
async fn received_reaction_independent_discovery_gate_and_retained_execution() {
    for native in [Native::Telegram, Native::Discord] {
        let mut f = Fixture::new(native, true, true).await;
        assert!(
            !f.app.state.telegram_effects_issuance_enabled
                && !f.app.state.discord_effects_issuance_enabled
        );
        assert!(crate::mcp::catalog::view(&f.app.state)
            .iter()
            .any(|t| t.name == "setMessageReaction"));
        let reference = f.reference().await;
        assert!(reference
            .open(&f.app.state.keys)
            .unwrap()
            .payload
            .consumer
            .is_some());
        f.gate(false);
        assert!(f.context().await.received_message.is_none());
        assert!(!crate::mcp::catalog::view(&f.app.state)
            .iter()
            .any(|t| t.name == "setMessageReaction"));
        assert_eq!(
            f.app
                .state
                .set_message_reaction(f.app.caller(), &reference, "retained", request("👍"))
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        let other = match native {
            Native::Telegram => Native::Discord,
            Native::Discord => Native::Telegram,
        };
        assert!(!f.app.state.received_reaction_enabled(other));
        assert_eq!(f.writes(), 1);
    }
}

#[tokio::test]
async fn received_reaction_context_requires_original_retained_binding_without_breaking_context() {
    for native in [Native::Telegram, Native::Discord] {
        let mut f = Fixture::new(native, false, false).await;
        let event = EventRef::seal(&f.app.event, &f.app.state.keys).unwrap();
        assert!(f
            .app
            .state
            .context(f.app.caller(), &event)
            .await
            .unwrap()
            .received_message
            .is_some());
        assert!(f
            .app
            .state
            .received_message_context(f.app.caller(), &f.app.event, &f.app.activity, false)
            .await
            .is_none());
        f.app.event.partition_key.push_str(":unbound");
        let unbound = EventRef::seal(&f.app.event, &f.app.state.keys).unwrap();
        for view in [
            f.app.state.context(f.app.caller(), &unbound).await.unwrap(),
            f.context().await,
        ] {
            assert!(view.activity_verified);
            assert!(view.received_message.is_none());
            assert!(serde_json::to_value(view)
                .unwrap()
                .get("received_message")
                .is_none());
        }
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn received_reaction_custom_emoji_and_platform_specific_is_big() {
    for native in [Native::Telegram, Native::Discord] {
        let f = Fixture::new(native, false, true).await;
        let reference = f.reference().await;
        let mut custom = request("");
        custom.reaction.push(TelegramReaction::CustomEmoji {
            custom_emoji_id: "500".into(),
        });
        custom.is_big = true;
        if native == Native::Discord {
            assert!(f
                .app
                .state
                .set_message_reaction(f.app.caller(), &reference, "big", custom.clone())
                .await
                .is_err());
            assert_eq!(f.writes(), 0);
            custom.is_big = false;
        }
        assert_eq!(
            f.app
                .state
                .set_message_reaction(f.app.caller(), &reference, "custom", custom)
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
        if native == Native::Telegram {
            assert_eq!(f.telegram.calls.lock().unwrap()[0]["is_big"], true);
            assert_eq!(
                f.telegram.calls.lock().unwrap()[0]["reaction"],
                json!([{"type":"custom_emoji","custom_emoji_id":"500"}])
            );
        } else {
            assert_eq!(
                f.discord.memory.lock().unwrap().writes[0].1,
                "/api/v10/channels/99/messages/800/reactions/party:500/@me"
            );
        }
    }
}

#[tokio::test]
async fn received_reaction_slow_retained_metadata_omits_capability_and_preserves_context() {
    use std::{sync::atomic::Ordering, time::Duration};
    for native in [Native::Telegram, Native::Discord] {
        let f = Fixture::new(native, false, false).await;
        f.reference().await;
        f.app.backend.get_delay_ms.store(2500, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let context = f.context().await;
        assert!(context.activity_verified && context.received_message.is_none());
        assert!(started.elapsed() < Duration::from_secs(4));
        f.app.backend.get_delay_ms.store(0, Ordering::SeqCst);
        assert!(f.context().await.received_message.is_some());
        assert_eq!(f.writes(), 0);
    }
}

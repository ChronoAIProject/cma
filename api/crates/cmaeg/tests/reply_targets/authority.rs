use super::support::{now, receipt, App};
use cmaeg_core::{
    channel::Source,
    seal::{EventRef, ReplyTargetRef},
};
use serde_json::json;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn target_expiry_tampering_and_wrong_key_are_refused_but_original_event_may_expire() {
    let app = App::new(vec![]).await;
    let event = app.event("alice", "expiry").await;
    let target = app.target(&event).await;
    let reference: ReplyTargetRef = target.parse().unwrap();
    let payload = reference.open(&app.state.keys, now()).unwrap().payload;
    let mut expired = payload.clone();
    expired.issued_at = now() - 60;
    expired.expires_at = now() - 1;
    let expired = ReplyTargetRef::seal(&expired, &app.state.keys).unwrap();
    assert_eq!(
        app.send(expired.as_str(), "id", "no")
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    let mut tampered = target.clone();
    let index = tampered.len() - 8;
    let replacement = if &tampered[index..index + 1] == "A" {
        "B"
    } else {
        "A"
    };
    tampered.replace_range(index..index + 1, replacement);
    assert_eq!(
        app.send(&tampered, "id", "no")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let wrong = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    assert_eq!(
        app.client
            .post(format!("{}/v1/reply-targets/{target}/messages", app.base))
            .bearer_auth(wrong)
            .header("Idempotency-Key", "id")
            .json(&json!({"text":"no"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut long = payload.clone();
    long.expires_at = long.issued_at + 86401;
    let long = ReplyTargetRef::seal(&long, &app.state.keys).unwrap();
    assert_eq!(
        app.send(long.as_str(), "id", "no")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut delayed = payload;
    delayed.issued_at = now() - 900;
    delayed.expires_at = delayed.issued_at + 86400;
    delayed.event.expires_at = now() - 300;
    let expired_event = EventRef::seal(&delayed.event, &app.state.keys).unwrap();
    assert!(expired_event.open(&app.state.keys, now()).is_err());
    let delayed = ReplyTargetRef::seal(&delayed, &app.state.keys).unwrap();
    let sent = receipt(
        app.send(delayed.as_str(), "id", "later")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(sent["delivery"]["outcome"], "confirmed");
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn current_route_binding_key_and_channel_existence_fence_even_receipt_replay() {
    for change in ["route", "binding", "key", "deleted"] {
        let mut app = App::new(vec![]).await;
        let target = app.target(&app.event("alice", "revoked").await).await;
        receipt(app.send(&target, "same", "first").send().await.unwrap()).await;
        match change {
            "route" => {
                if let Source::NyxidRelay { route_ids, .. } = &mut app.definition.sources[0] {
                    route_ids.retain(|id| id != "route-alice");
                }
            }
            "binding" => app.definition.provider.binding_id = "different-binding".parse().unwrap(),
            "key" => {
                app.key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
                app.definition.secrets.agent_key_hash =
                    format!("sha256:{:x}", Sha256::digest(app.key.as_bytes()));
            }
            "deleted" => {}
            _ => unreachable!(),
        }
        if change == "deleted" {
            app.client
                .delete(
                    app.records
                        .record_url(app.definition.record_id.as_str())
                        .unwrap(),
                )
                .bearer_auth("cma-service")
                .query(&[("expected_version", "1")])
                .send()
                .await
                .unwrap();
        } else {
            app.update().await;
        }
        let response = app.send(&target, "same", "first").send().await.unwrap();
        assert_eq!(
            response.status(),
            if change == "deleted" { 404 } else { 403 },
            "{change}"
        );
        assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn mint_requires_verified_context_and_cannot_accept_a_caller_selected_destination() {
    let app = App::new(vec![]).await;
    let event = app.event("alice", "mint").await;
    let url = format!("{}/v1/events/{}/reply-target", app.base, event.as_str());
    assert_eq!(
        app.client
            .post(&url)
            .bearer_auth(&app.key)
            .json(&json!({"route_id":"route-bob"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut old = event.open(&app.state.keys, now()).unwrap().payload;
    old.activity_sha256 = None;
    let old = EventRef::seal(&old, &app.state.keys).unwrap();
    assert_eq!(
        app.client
            .post(format!(
                "{}/v1/events/{}/reply-target",
                app.base,
                old.as_str()
            ))
            .bearer_auth(&app.key)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let target = app.target(&event).await;
    let response = app
        .client
        .post(format!("{}/v1/reply-targets/{target}/messages", app.base))
        .bearer_auth(&app.key)
        .json(&json!({"text":"missing key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(app.relay.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_reply_target_survives_restart_and_refuses_changed_bot_before_dispatch() {
    let mut app = App::new(vec![]).await;
    app.definition.sources = vec![Source::Telegram {
        topic_lifecycle: false,
        topic_creation: false,
        message_events: false,
        channel_posts: false,
        member_join_chat_ids: Vec::new(),
        bot_id: "77".into(),
        bot_username: Some("fixture_bot".into()),
        admission: cmaeg_core::channel::AdmissionPolicy::All {},
    }];
    app.definition.secrets.telegram_bot_token =
        Some(cmaeg_core::channel::Secret::new("77:fixture"));
    app.definition.secrets.telegram_webhook_secret =
        Some(cmaeg_core::channel::Secret::new("webhook-fixture"));
    app.update().await;
    let event = app.event("42", "native-target").await;
    let target = app.target(&event).await;
    app.restart().await;
    let reopened = target
        .parse::<ReplyTargetRef>()
        .unwrap()
        .open(&app.state.keys, now())
        .unwrap();
    assert!(
        matches!(reopened.payload.event.reply,cmaeg_core::seal::ReplyAddress::Telegram{chat_id,bot_id,..}if chat_id=="42"&&bot_id=="77")
    );
    if let Source::Telegram { bot_id, .. } = &mut app.definition.sources[0] {
        *bot_id = "88".into();
    }
    app.definition.secrets.telegram_bot_token =
        Some(cmaeg_core::channel::Secret::new("88:fixture"));
    app.update().await;
    assert_eq!(
        app.send(&target, "late", "must not dispatch")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(app.relay.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_discord_and_whatsapp_targets_survive_restart_and_fence_source_changes() {
    for discord in [true, false] {
        let mut app = App::new(vec![]).await;
        if discord {
            app.definition.sources = vec![cmaeg_core::channel::Source::Discord {
                bot_id: "77".into(),
                intents: 37377,
                thread_history: false,
                thread_lifecycle: false,
                conversation_history: false,
                admission: cmaeg_core::channel::AdmissionPolicy::All {},
            }];
            app.definition.secrets.discord_bot_token =
                Some(cmaeg_core::channel::Secret::new("fixture-discord-token"));
        } else {
            app.definition.sources = vec![cmaeg_core::channel::Source::Whatsapp {
                message_events: false,
                forms_enabled: false,
                group_ids: vec![],
                app_id: "12345".into(),
                waba_id: "23456".into(),
                phone_number_id: "34567".into(),
                admission: cmaeg_core::channel::AdmissionPolicy::All {},
            }];
            app.definition.secrets.whatsapp = Some(cmaeg_core::channel::WhatsappCredentials {
                app_secret: cmaeg_core::channel::Secret::new("fixture-app-secret-12345"),
                access_token: cmaeg_core::channel::Secret::new("fixture-access-token-12345"),
                verify_token: cmaeg_core::channel::Secret::new("fixture-verify-token-12345"),
            });
        }
        app.update().await;
        let actor = if discord { "99" } else { "447700900123" };
        let event = app
            .event(actor, if discord { "discord" } else { "whatsapp" })
            .await;
        let target = app.target(&event).await;
        app.restart().await;
        let reopened = target
            .parse::<ReplyTargetRef>()
            .unwrap()
            .open(&app.state.keys, now())
            .unwrap();
        match reopened.payload.event.reply {
            cmaeg_core::seal::ReplyAddress::Discord {
                bot_id,
                channel_id,
                message_id,
                ..
            } if discord => {
                assert_eq!(
                    (bot_id.as_str(), channel_id.as_str(), message_id.as_str()),
                    ("77", "99", "700")
                );
            }
            cmaeg_core::seal::ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } if !discord => {
                assert_eq!(
                    (
                        app_id.as_str(),
                        waba_id.as_str(),
                        phone_number_id.as_str(),
                        recipient_id.as_str(),
                        message_id.as_str()
                    ),
                    ("12345", "23456", "34567", "447700900123", "wamid.original")
                );
            }
            reply => panic!("unexpected native reply target: {reply:?}"),
        }
        if discord {
            if let cmaeg_core::channel::Source::Discord { bot_id, .. } =
                &mut app.definition.sources[0]
            {
                *bot_id = "88".into();
            }
            app.definition.secrets.discord_bot_token =
                Some(cmaeg_core::channel::Secret::new("fixture-discord-token-88"));
        } else if let cmaeg_core::channel::Source::Whatsapp {
            phone_number_id, ..
        } = &mut app.definition.sources[0]
        {
            *phone_number_id = "34568".into();
        }
        app.update().await;
        assert_eq!(
            app.send(&target, "native-change", "must not dispatch")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert!(app.relay.requests.lock().unwrap().is_empty());
    }
}

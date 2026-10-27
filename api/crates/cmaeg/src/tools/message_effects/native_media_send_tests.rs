//! Exercise optional edit enrichment through the real delivery engine.
use super::*;
use crate::{providers::EventContextSnapshot, turns::routed::adapter_test_server::HttpServer};

async fn enable(app: &mut App, telegram: bool) -> HttpServer {
    let provider = HttpServer::start().await;
    provider.memory.lock().unwrap().config.event_context = Some(
        serde_json::to_value(EventContextSnapshot {
            schema_version: 1,
            channel_id: app.event.channel_id.clone(),
            conversation_id: app.event.conversation_id.clone(),
            activity: app.activity.clone(),
            event_ref: EventRef::seal(&app.event, &app.state.keys).unwrap(),
        })
        .unwrap(),
    );
    app.state.providers = provider.client(true, Duration::from_secs(2));
    let previous = app.state.native_media_edits.as_ref().unwrap();
    app.state.native_media_edits = Some(Arc::new(crate::native_media::Client::new(
        previous.telegram.clone(),
        previous.discord.clone(),
        telegram,
        !telegram,
    )));
    app.media_effects = true;
    app.restart().await;
    provider
}
fn upload(caption: &str, text: Option<&str>) -> cmaeg_core::message::OutboundMessage {
    let mut body = json!({"native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"report.bin","mime_type":"application/octet-stream","caption":caption}]});
    if let Some(text) = text {
        body["text"] = text.into();
    }
    serde_json::from_value(body).unwrap()
}

#[tokio::test]
async fn discord_optional_edit_enrichment_preserves_confirmed_delivery_and_old_effects() {
    for scenario in ["plain", "embed", "storage", "wrong_bot"] {
        let (mut app, _) = media_app().await;
        let _provider = enable(&mut app, false).await;
        {
            let mut memory = app.platform.memory.lock().unwrap();
            memory.message["content"] = "https://example.org".into();
            memory.message["attachments"][0]["description"] = "alt".into();
            if scenario == "embed" {
                memory.message["embeds"] = json!([{"type":"link","url":"https://example.org"}]);
            }
            if scenario == "wrong_bot" {
                memory.message["author"]["id"] = "78".into();
            }
        }
        if scenario == "storage" {
            app.backend.fail_put.store(true, Ordering::SeqCst);
        }
        let receipt = app
            .send_message(upload("alt", Some("https://example.org")))
            .await;
        assert_eq!(
            app.platform.memory.lock().unwrap().writes.len(),
            1,
            "{scenario}"
        );
        if scenario == "wrong_bot" {
            assert_eq!(receipt.delivery.kind(), "unknown");
            assert!(receipt.message_ref.is_none());
            continue;
        }
        assert_eq!(receipt.delivery.kind(), "confirmed", "{scenario}");
        let reference = receipt.message_ref.unwrap();
        let opened = reference.open(&app.state.keys).unwrap();
        assert_eq!(
            opened.payload.native_media_edit.is_some(),
            scenario == "plain",
            "{scenario}"
        );
        assert!(opened
            .payload
            .discord_message
            .as_ref()
            .unwrap()
            .effects
            .is_some());
        if scenario != "storage" {
            let capabilities = app.caps(&reference).await;
            assert_eq!(
                capabilities.get("media_edit").is_some(),
                scenario == "plain"
            );
        }
    }
}

#[tokio::test]
async fn telegram_optional_edit_enrichment_preserves_confirmed_caption_entities_and_storage_failure(
) {
    for scenario in ["plain", "entities", "storage", "wrong_bot"] {
        let (mut app, _, platform) = telegram_app().await;
        let _provider = enable(&mut app, true).await;
        let caption = "https://example.org";
        let mut body = telegram_body(caption);
        if scenario == "entities" {
            body["caption_entities"] = json!([{"type":"url","offset":0,"length":19}]);
        }
        if scenario == "wrong_bot" {
            body["from"]["id"] = 78.into();
        }
        answer_telegram(&platform, body);
        if scenario == "storage" {
            app.backend.fail_put.store(true, Ordering::SeqCst);
        }
        let receipt = app.send_message(upload(caption, None)).await;
        assert_eq!(
            platform
                .calls()
                .iter()
                .filter(|call| call.path.ends_with("/sendDocument"))
                .count(),
            1,
            "{scenario}"
        );
        if scenario == "wrong_bot" {
            assert_eq!(receipt.delivery.kind(), "unknown");
            assert!(receipt.message_ref.is_none());
            continue;
        }
        assert_eq!(receipt.delivery.kind(), "confirmed", "{scenario}");
        let opened = receipt.message_ref.unwrap().open(&app.state.keys).unwrap();
        assert_eq!(
            opened.payload.native_media_edit.is_some(),
            scenario == "plain",
            "{scenario}"
        );
        assert!(opened.payload.telegram_effect.is_some());
    }
}

#[tokio::test]
async fn optional_context_outage_skips_edit_enrichment_but_context_mismatch_refuses() {
    for contradictory in [false, true] {
        let (mut app, _) = media_app().await;
        let provider = enable(&mut app, false).await;
        if contradictory {
            provider
                .memory
                .lock()
                .unwrap()
                .config
                .event_context
                .as_mut()
                .unwrap()["channel_id"] = "cmaeg1.ch.wrong".into();
        } else {
            provider.memory.lock().unwrap().config.event_context = None;
        }
        let receipt = app.send_message(upload("alt", None)).await;
        assert_eq!(
            receipt.delivery.kind(),
            if contradictory {
                "rejected"
            } else {
                "confirmed"
            }
        );
        assert_eq!(
            app.platform.memory.lock().unwrap().writes.len(),
            usize::from(!contradictory)
        );
        if let Some(reference) = receipt.message_ref {
            assert!(reference
                .open(&app.state.keys)
                .unwrap()
                .payload
                .native_media_edit
                .is_none());
        }
    }
}

#[tokio::test]
async fn optional_telegram_identity_outage_skips_edit_enrichment_without_retrying_send() {
    let (mut app, _, platform) = telegram_app().await;
    let _provider = enable(&mut app, true).await;
    platform.answer(crate::telegram::media_fixture::Answer::json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"ok":false}),
    ));
    platform.answer(crate::telegram::media_fixture::Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":telegram_body("caption")}),
    ));
    let receipt = app.send_message(upload("caption", None)).await;
    assert_eq!(receipt.delivery.kind(), "confirmed");
    assert!(receipt
        .message_ref
        .unwrap()
        .open(&app.state.keys)
        .unwrap()
        .payload
        .native_media_edit
        .is_none());
    assert_eq!(
        platform
            .calls()
            .iter()
            .filter(|call| call.path.ends_with("/sendDocument"))
            .count(),
        1
    );
}

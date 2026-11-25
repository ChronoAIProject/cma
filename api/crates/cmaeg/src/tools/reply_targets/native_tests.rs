use super::native_fixture::{App, Variant, VARIANTS};
use crate::delivery::intents::DeliveryOutcome;
use serde_json::Value;

#[tokio::test]
async fn native_reply_targets_deliver_exact_destinations_after_restart_and_event_expiry() {
    for variant in VARIANTS {
        let mut app = App::new(variant, false).await;
        let target = app.target().await;
        app.restart();
        tokio::time::sleep(std::time::Duration::from_secs(
            (app.expiry - super::now() + 1).max(0) as u64,
        ))
        .await;
        assert!(app.event.open(&app.state.keys, super::now()).is_err());
        let first = app.send(&target, "result-1").await;
        assert!(
            matches!(first.delivery, DeliveryOutcome::Confirmed { .. }),
            "{variant:?}: {:?}",
            first.delivery
        );
        app.restart();
        let replay = app.send(&target, "result-1").await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        assert_eq!(app.sends(), 1, "{variant:?}");
        match variant {
            Variant::Discord | Variant::DiscordThread => {
                let calls = app.discord.memory.calls.lock().unwrap();
                let call = calls.iter().find(|call| call.method == "POST").unwrap();
                let destination = if matches!(variant, Variant::DiscordThread) {
                    "101"
                } else {
                    "99"
                };
                assert_eq!(
                    call.path,
                    format!("/api/v10/channels/{destination}/messages")
                );
                let body: Value = serde_json::from_slice(&call.body).unwrap();
                assert_eq!(body["message_reference"]["message_id"], "700");
                assert_eq!(body["message_reference"]["channel_id"], destination);
                assert!(app.graph.calls.lock().unwrap().is_empty());
            }
            Variant::Whatsapp | Variant::WhatsappGroup => {
                let calls = app.graph.calls.lock().unwrap();
                let (_, url, body) = calls
                    .iter()
                    .find(|(method, _, _)| *method == reqwest::Method::POST)
                    .unwrap();
                assert_eq!(url.path(), "/v25.0/34567/messages");
                let body: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(
                    body["to"],
                    if matches!(variant, Variant::WhatsappGroup) {
                        crate::whatsapp::groups::tests::GROUP
                    } else {
                        "447700900123"
                    }
                );
                assert_eq!(body["context"]["message_id"], "wamid.original");
                assert_eq!(
                    body["recipient_type"],
                    if matches!(variant, Variant::WhatsappGroup) {
                        "group"
                    } else {
                        "individual"
                    }
                );
                assert!(app.discord.memory.calls.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn native_reply_targets_lost_response_fences_replacement_keys_and_reminting() {
    for variant in VARIANTS {
        let mut app = App::new(variant, true).await;
        let target = app.target().await;
        let reminted = app.target().await;
        let first = app.send(&target, "original-key").await;
        assert!(
            matches!(first.delivery, DeliveryOutcome::Unknown { .. }),
            "{variant:?}: {:?}",
            first.delivery
        );
        app.restart();
        let replay = app.send(&target, "original-key").await;
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        let replacement = app.send(&reminted, "replacement-key").await;
        assert!(
            matches!(replacement.delivery, DeliveryOutcome::Unknown { .. }),
            "{variant:?}"
        );
        assert_eq!(app.sends(), 1, "{variant:?}");
    }
}

#[tokio::test]
async fn native_reply_targets_refuse_revoked_source_without_transport_fallback() {
    for variant in VARIANTS {
        let mut app = App::new(variant, false).await;
        let target = app.target().await;
        app.revoke();
        app.restart();
        let result = app
            .state
            .reply_to_target(
                app.caller,
                &target,
                "result-1",
                serde_json::from_value(serde_json::json!({"text":"approved result"})).unwrap(),
            )
            .await;
        assert!(result.is_err(), "{variant:?}");
        assert_eq!(app.sends(), 0, "{variant:?}");
    }
}

#[tokio::test]
async fn event_side_message_has_its_own_receipt_and_leaves_native_answer_authority() {
    for failure in [false, true] {
        let app = App::new(Variant::DiscordThread, failure).await;
        let side_messages = app.side_messages();
        assert!(app
            .state
            .side_message_available(app.caller, &app.event)
            .await
            .unwrap());
        let first = app
            .state
            .send_side_message(
                app.caller,
                &app.event,
                "same-key",
                &side_messages,
                "progress".into(),
            )
            .await
            .unwrap();
        if failure {
            assert!(matches!(first.delivery, DeliveryOutcome::Unknown { .. }));
        } else {
            assert!(matches!(first.delivery, DeliveryOutcome::Confirmed { .. }));
        }
        let replay = app
            .state
            .send_side_message(
                app.caller,
                &app.event,
                "same-key",
                &side_messages,
                "progress".into(),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        let edited = app
            .state
            .send_side_message(
                app.caller,
                &app.event,
                "same-key",
                &side_messages,
                "changed".into(),
            )
            .await
            .unwrap_err();
        assert_eq!(edited.status, 409);
        assert_eq!(app.sends(), 1);
        assert!(!app
            .state
            .tracker
            .has_tool_reply(app.event.as_str(), super::now()));

        *app.discord.memory.broken.lock().unwrap() = false;
        let reply = app.target().await;
        let final_answer = app.send(&reply, "same-key").await;
        assert!(matches!(
            final_answer.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert_eq!(app.sends(), 2);
        let calls = app.discord.memory.calls.lock().unwrap();
        assert!(calls
            .iter()
            .filter(|call| call.method == "POST")
            .all(|call| call.path == "/api/v10/channels/101/messages"));
    }
}

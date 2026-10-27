use super::*;

fn form() -> Value {
    json!({"blocks":[{"type":"form","id":"request","fields":[
        {"id":"private-reason","label":"Reason","required":true,"kind":{"type":"text"}}
    ]}]})
}
fn recording(setup: &Setup, enabled: bool) -> Recording {
    Recording::start_with_relay(
        FakeHttp::new(|request, _| {
            if request.url.path().ends_with("reply/update") {
                return Answer::json(
                    200,
                    json!({"upstream_message_id":"om_card","edited_at":"2026-10-27T00:00:00Z"}),
                );
            }
            Answer::json(
                200,
                json!({"message_id":"outbound-1","platform_message_id":"om_card"}),
            )
        }),
        Hooks::answering([]),
        DeliveryConfig::default(),
        Some(setup.store.clone()),
        false,
        |relay| relay.with_lark_cards(true).with_lark_forms(enabled),
    )
}

#[tokio::test]
async fn native_lark_forms_off_preserves_legacy_text_and_mixed_card_degradation() {
    for platform in [Platform::Lark, Platform::Feishu] {
        let setup = Setup::new(platform).await;
        let mut legacy = setup.recording(false, false);
        let before = legacy
            .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
            .await;
        let expected = legacy.http.sent()[0].json();
        legacy.finish().await;
        let mut default = recording(&setup, false);
        let after = default
            .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
            .await;
        assert_eq!(after.delivery, before.delivery);
        assert_eq!(after.rendering, before.rendering);
        assert_eq!(default.http.sent()[0].json(), expected);
        assert!(setup.backend.records().is_empty());
        assert!(!setup.provider.log().iter().any(|r| r.method == "GET"));
        default.finish().await;
        let mut mixed = form();
        mixed["blocks"]
            .as_array_mut()
            .unwrap()
            .extend(choice()["blocks"].as_array().unwrap().iter().cloned());
        let mut default = recording(&setup, false);
        let receipt = default
            .reply(
                &setup.channel,
                &setup.event,
                mixed.clone(),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let body = default.http.sent()[0].json();
        let elements = body["reply"]["metadata"]["card"]["body"]["elements"]
            .as_array()
            .unwrap();
        assert_eq!(
            elements[0],
            json!({"tag":"div","text":{"tag":"plain_text","content":"Reason: (required)"}})
        );
        assert!(!elements.iter().any(|e| e["tag"] == "form"));
        assert_eq!(
            serde_json::to_value(receipt.rendering).unwrap()["blocks"][0]["outcome"]["how"],
            "plain_text_form"
        );
        let reference = receipt.message_ref.unwrap().open(&keys()).unwrap().payload;
        assert!(setup
            .store
            .load(reference.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0
            .lark
            .unwrap()
            .form
            .is_none());
        let mut click = setup.activity.clone();
        click.event_id = "click-1".parse().unwrap();
        setup
            .store
            .consume(
                &setup.channel,
                &click,
                "route-1",
                "query-1",
                &token(&body),
                "om_card",
                &|c| c.is_none(),
            )
            .await
            .unwrap();
        default.finish().await;
        // Enabling issuance does not silently convert a historical plain form during edit.
        let mut enabled = recording(&setup, true);
        let edited = enabled.update(&setup.channel, reference, mixed).await;
        assert!(matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }));
        assert!(!enabled.http.sent()[0].json().to_string().contains("clf1:"));
        enabled.finish().await;
    }
}

#[tokio::test]
async fn issued_native_lark_forms_remain_editable_with_issuance_off() {
    for platform in [Platform::Lark, Platform::Feishu] {
        let setup = Setup::new(platform).await;
        let mut enabled = recording(&setup, true);
        let reply = enabled
            .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
            .await;
        assert!(matches!(reply.delivery, DeliveryOutcome::Confirmed { .. }));
        let body = enabled.http.sent()[0].json();
        let elements = &body["reply"]["metadata"]["card"]["body"]["elements"];
        assert_eq!(elements[0]["tag"], "form");
        assert_eq!(elements[0]["elements"][1]["name"], "f0");
        assert!(!body.to_string().contains("private-reason"));
        let reference = reply.message_ref.unwrap().open(&keys()).unwrap().payload;
        let id = reference.interaction_record_id.as_ref().unwrap();
        let (mut card, _) = setup.store.load(id).await.unwrap();
        // The signed callback suite covers the actual consume path. Seed its persisted
        // result here to isolate delivery's restart/gate-off replacement behavior.
        card.consumed = Some(
            serde_json::from_value(
                json!({"event_id":"click-1","callback_id":"query-1","action":0,
            "occurred_at":"2026-10-27T00:00:00Z","submitted":{"private-reason":"Ready"}}),
            )
            .unwrap(),
        );
        setup.backend.seed(
            id,
            &setup.channel.definition.owner.subject,
            cmaeg_core::seal::seal_interaction(&keys(), id, &serde_json::to_vec(&card).unwrap())
                .unwrap(),
        );
        enabled.finish().await;
        let mut stopped = recording(&setup, false);
        let edited = stopped.update(&setup.channel, reference, form()).await;
        assert!(
            matches!(edited.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            edited.delivery
        );
        assert_eq!(
            stopped.http.sent()[0].json()["reply"]["metadata"]["card"]["body"]["elements"][0]
                ["tag"],
            "form"
        );
        let next = edited.message_ref.unwrap().open(&keys()).unwrap().payload;
        assert!(setup
            .store
            .load(next.interaction_record_id.as_ref().unwrap())
            .await
            .unwrap()
            .0
            .lark
            .unwrap()
            .form
            .is_some());
        assert!(setup.store.load(&card.id).await.unwrap().0.superseded);
        stopped.finish().await;
    }
}

#[tokio::test]
async fn named_text_form_edits_keep_telegram_and_plain_relay_compatibility() {
    use cmaeg_core::seal::{MessageRefPayload, RelayMessageDestination};
    for platform in [
        None,
        Some(Platform::Telegram),
        Some(Platform::Lark),
        Some(Platform::Feishu),
    ] {
        let setup = Setup::new(platform.unwrap_or(Platform::Telegram)).await;
        let channel = if platform.is_none() {
            super::super::recording_fixtures::native_channel()
        } else {
            setup.channel.clone()
        };
        let reference = MessageRefPayload {
            discord_message: None,
            telegram_effect: None,
            channel_id: channel.id.clone(),
            event_id: "msg-1".parse().unwrap(),
            platform_message_id: if platform.is_none() { "100" } else { "om_card" }.into(),
            route_id: if platform.is_none() {
                "telegram:77:42"
            } else {
                "route-1"
            }
            .into(),
            interaction_record_id: None,
            consumer: Some(cmaeg_core::ids::ConsumerId::parse("csm_00000000000a").unwrap()),
            relay_message: platform.map(|platform| RelayMessageDestination {
                platform,
                outbound_message_id: "outbound-1".into(),
            }),
        };
        let mut recorder = Recording::start_with_relay(
            FakeHttp::new(|request, _| {
                if request.url.path().ends_with("reply/update") {
                    Answer::json(
                        200,
                        json!({"upstream_message_id":"om_card","edited_at":"2026-10-27T00:00:00Z"}),
                    )
                } else {
                    Answer::json(
                        200,
                        json!({"ok":true,"result":{"message_id":100,"chat":{"id":42}}}),
                    )
                }
            }),
            Hooks::answering([]),
            DeliveryConfig::default(),
            Some(setup.store.clone()),
            false,
            |relay| {
                relay
                    .with_lark_cards(true)
                    .with_lark_forms(true)
                    .with_consumer_interactions(false)
            },
        );
        let receipt = recorder.update(&channel, reference, form()).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{platform:?}: {:?}",
            receipt.delivery
        );
        assert_eq!(recorder.http.sent().len(), 1);
        assert!(!recorder.http.sent()[0].json().to_string().contains("clf1:"));
        assert!(setup.backend.records().is_empty());
        recorder.finish().await;
    }
}

#[tokio::test]
async fn native_lark_form_unsupported_schema_refuses_before_context_read_record_or_send() {
    let setup = Setup::new(Platform::Lark).await;
    let mut recording = recording(&setup, true);
    for case in ["duplicate", "mixed", "threads", "media", "large"] {
        let mut message = form();
        match case {
            "duplicate" => {
                let field = message["blocks"][0]["fields"][0].clone();
                message["blocks"][0]["fields"]
                    .as_array_mut()
                    .unwrap()
                    .push(field);
            }
            "mixed" => message["blocks"].as_array_mut().unwrap().push(
                json!({"type":"buttons","id":"approval","options":[{"id":"yes","label":"Yes"}]}),
            ),
            "threads" => message["thread_reply"] = true.into(),
            "media" => {
                message["native_attachments"] =
                    json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}])
            }
            _ => message["text"] = "x".repeat(4001).into(),
        }
        let receipt = recording
            .reply(&setup.channel, &setup.event, message, ReplySource::Tool)
            .await;
        assert!(
            !matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{case}"
        );
        assert!(recording.http.sent().is_empty());
        assert!(setup.backend.records().is_empty());
        assert!(!setup.provider.log().iter().any(|r| r.method == "GET"));
    }
    recording.finish().await;
}

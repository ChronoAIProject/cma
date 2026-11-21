//! Form issuance is an optional rendering, not a restriction on legacy replies.
use super::*;
use cmaeg_core::{activity::ActorKind, message::OutboundMessage};
use std::sync::atomic::Ordering;

fn mixed(kind: &str) -> Value {
    let mut message = form();
    let mut control = choice()["blocks"][0].clone();
    control["type"] = kind.into();
    if kind == "choice" {
        control["prompt"] = "Approve?".into();
        control["multi"] = false.into();
    }
    message["blocks"].as_array_mut().unwrap().push(control);
    message
}

async fn same_text_fallback(setup: &Setup, message: Value) {
    let mut expected = None;
    for enabled in [false, true] {
        let mut recorder = recording(setup, enabled);
        let receipt = recorder
            .reply(
                &setup.channel,
                &setup.event,
                message.clone(),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let sent = recorder.http.sent();
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|r| r.json()["reply"]["metadata"].is_null()));
        // Include destinations and credentials, not just displayed text.
        let actual = (
            receipt.delivery,
            receipt.rendering,
            sent.iter()
                .map(|r| (r.url.clone(), r.headers.clone(), r.json()))
                .collect::<Vec<_>>(),
        );
        if let Some(expected) = &expected {
            assert_eq!(&actual, expected);
        } else {
            expected = Some(actual);
        }
        assert!(setup.backend.records().is_empty());
        recorder.finish().await;
    }
}

#[tokio::test]
async fn form_flag_preserves_mixed_buttons_and_choices_without_form_authority() {
    for platform in [Platform::Lark, Platform::Feishu] {
        for kind in ["buttons", "choice"] {
            let setup = Setup::new(platform).await;
            let mut expected = None;
            for enabled in [false, true] {
                let mut recorder = recording(&setup, enabled);
                let receipt = recorder
                    .reply(&setup.channel, &setup.event, mixed(kind), ReplySource::Tool)
                    .await;
                assert!(matches!(
                    receipt.delivery,
                    DeliveryOutcome::Confirmed { .. }
                ));
                assert_eq!(recorder.http.sent().len(), 1);
                let body = recorder.http.sent()[0].json();
                let locator = token(&body);
                assert!(locator.starts_with("cl1:"));
                assert!(!body.to_string().contains("clf1:"));
                assert_eq!(
                    serde_json::to_value(&receipt.rendering).unwrap()["blocks"][0]["outcome"]
                        ["how"],
                    "plain_text_form"
                );
                let actual = (
                    receipt.delivery,
                    receipt.rendering,
                    body.to_string()
                        .replace(locator.split(':').nth(1).unwrap(), "<record>"),
                );
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
                let reference = receipt.message_ref.unwrap().open(&keys()).unwrap().payload;
                let card = setup
                    .store
                    .load(reference.interaction_record_id.as_ref().unwrap())
                    .await
                    .unwrap()
                    .0;
                assert!(card.lark.unwrap().form.is_none());
                recorder.finish().await;
            }
            assert_eq!(setup.backend.records().len(), 2);
            assert_eq!(
                setup
                    .provider
                    .log()
                    .iter()
                    .filter(|r| r.method == "GET")
                    .count(),
                2
            );
        }
    }
}

#[tokio::test]
async fn unsupported_native_form_shapes_keep_valid_legacy_text_without_context_reads() {
    for platform in [Platform::Lark, Platform::Feishu] {
        for case in [
            "duplicate_fields",
            "multiple_forms",
            "choice_style",
            "large",
        ] {
            let setup = Setup::new(platform).await;
            let mut message = form();
            match case {
                "duplicate_fields" => {
                    let field = message["blocks"][0]["fields"][0].clone();
                    message["blocks"][0]["fields"]
                        .as_array_mut()
                        .unwrap()
                        .push(field);
                }
                "multiple_forms" => {
                    let mut second = message["blocks"][0].clone();
                    second["id"] = "second-form".into();
                    message["blocks"].as_array_mut().unwrap().push(second);
                }
                "choice_style" => {
                    message["blocks"][0]["fields"][0]["kind"] = json!({"type":"choice","options":[{"id":"yes","label":"Yes","style":"primary"}]})
                }
                _ => message["text"] = "x".repeat(4001).into(),
            }
            serde_json::from_value::<OutboundMessage>(message.clone())
                .unwrap()
                .validate()
                .unwrap();
            same_text_fallback(&setup, message).await;
            assert!(!setup.provider.log().iter().any(|r| r.method == "GET"));
        }
    }
}

#[tokio::test]
async fn ineligible_form_contexts_preserve_exact_text_and_reply_destination() {
    for platform in [Platform::Lark, Platform::Feishu] {
        for case in [
            "bot",
            "system",
            "unknown",
            "conversation_thread",
            "thread_facts",
            "thread_target",
            "lineage",
            "no_digest",
            "no_key",
            "named_gate_off",
        ] {
            let mut setup = Setup::with_activity(platform, |activity| match case {
                "bot" => activity.actor.kind = ActorKind::Bot,
                "system" => activity.actor.kind = ActorKind::System,
                "unknown" => activity.actor.kind = ActorKind::Unknown,
                "conversation_thread" => activity.conversation.thread_id = Some("thread-1".into()),
                "thread_facts" => {
                    activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
                    activity.thread = Some(
                        serde_json::from_value(json!({
                            "version":1, "kind":"native", "chat_id":"oc_chat",
                            "message_id":"inbound-1", "native_thread_id":"thread-1",
                            "sender_kind":"human", "address":"mention"
                        }))
                        .unwrap(),
                    );
                }
                _ => {}
            })
            .await;
            match case {
                "thread_target" => {
                    let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut setup.event.reply else { panic!() };
                    *thread_reply = true;
                }
                "lineage" => setup.event.lineage = Some(serde_json::from_value(json!({"root":"00000000000000000000000000000001","hop":1,"path":["default"],"reply":"origin"})).unwrap()),
                "no_digest" => setup.event.activity_sha256 = None,
                "no_key" => Arc::get_mut(&mut setup.channel).unwrap().definition.secrets.channel_key = None,
                "named_gate_off" => setup.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse("csm_00000000000a").unwrap()),
                _ => {}
            }
            for mut message in [form(), mixed("buttons"), mixed("choice")] {
                if case == "thread_target" {
                    message["thread_reply"] = true.into();
                }
                same_text_fallback(&setup, message).await;
            }
            let reads = setup
                .provider
                .log()
                .iter()
                .filter(|r| r.method == "GET")
                .count();
            assert_eq!(
                reads,
                if matches!(
                    case,
                    "bot" | "system" | "unknown" | "conversation_thread" | "thread_facts"
                ) {
                    5
                } else {
                    0
                },
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn native_forms_require_both_card_gate_and_interaction_store() {
    for cards in [false, true] {
        let setup = Setup::new(Platform::Lark).await;
        let mut expected = None;
        for enabled in [false, true] {
            let mut recorder = Recording::start_with_relay(
                FakeHttp::relay(),
                Hooks::answering([]),
                DeliveryConfig::default(),
                (!cards).then(|| setup.store.clone()),
                false,
                |relay| relay.with_lark_cards(cards).with_lark_forms(enabled),
            );
            let receipt = recorder
                .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
                .await;
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert_eq!(recorder.http.sent().len(), 1);
            let body = recorder.http.sent()[0].json();
            assert!(body["reply"]["metadata"].is_null());
            let actual = (receipt.delivery, receipt.rendering, body);
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
            recorder.finish().await;
        }
        assert!(setup.backend.records().is_empty());
        assert!(!setup.provider.log().iter().any(|r| r.method == "GET"));
    }
}

#[tokio::test]
async fn form_flag_keeps_existing_thread_and_media_refusals() {
    for thread in [false, true] {
        let setup = Setup::new(Platform::Lark).await;
        let mut message = form();
        if thread {
            message["thread_reply"] = true.into();
        } else {
            message["native_attachments"] =
                json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}]);
        }
        let mut expected = None;
        for enabled in [false, true] {
            let mut recorder = recording(&setup, enabled);
            let receipt = recorder
                .reply(
                    &setup.channel,
                    &setup.event,
                    message.clone(),
                    ReplySource::Tool,
                )
                .await;
            assert!(!matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let actual = (receipt.delivery, receipt.rendering);
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected);
            } else {
                expected = Some(actual);
            }
            assert!(recorder.http.sent().is_empty());
            recorder.finish().await;
        }
        assert!(setup.backend.records().is_empty());
        assert!(!setup.provider.log().iter().any(|r| r.method == "GET"));
    }
}

#[tokio::test]
async fn native_form_authorization_context_and_storage_errors_never_fall_back() {
    for case in ["digest", "context", "credential", "owner", "storage"] {
        let mut setup = Setup::new(Platform::Lark).await;
        match case {
            "digest" => setup.event.activity_sha256 = Some("0".repeat(64)),
            "context" => setup.event.event_id = "missing-context".parse().unwrap(),
            "credential" => {
                Arc::get_mut(&mut setup.channel)
                    .unwrap()
                    .definition
                    .secrets
                    .provider_credential = None
            }
            "owner" => {
                let ReplyAddress::NyxidRelay { route_id, .. } = &mut setup.event.reply else {
                    panic!()
                };
                *route_id = "other-route".into();
            }
            _ => setup.backend.fail_put.store(true, Ordering::SeqCst),
        }
        let mut recorder = recording(&setup, true);
        let receipt = recorder
            .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
            .await;
        assert!(
            !matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{case}"
        );
        assert!(recorder.http.sent().is_empty(), "{case}");
        assert!(setup.backend.records().is_empty(), "{case}");
        recorder.finish().await;
    }
}

#[tokio::test]
async fn uncertain_native_form_send_or_binding_never_falls_back_or_resends() {
    for case in ["receipt", "server", "transport", "binding"] {
        let setup = Setup::new(Platform::Lark).await;
        let fail_put = setup.backend.fail_put.clone();
        let http = FakeHttp::new(move |_, _| match case {
            "receipt" => Answer::json(200, json!({"message_id":"outbound-1"})),
            "server" => Answer::status(503),
            "transport" => Answer {
                reply: Err(cmaeg::net::exchange::TransportError::Unavailable),
                delay: Duration::ZERO,
            },
            _ => {
                fail_put.store(true, Ordering::SeqCst);
                Answer::json(
                    200,
                    json!({"message_id":"outbound-1","platform_message_id":"om_card"}),
                )
            }
        });
        let mut recorder = Recording::start_with_relay(
            http,
            Hooks::answering([]),
            DeliveryConfig::default(),
            Some(setup.store.clone()),
            false,
            |relay| relay.with_lark_cards(true).with_lark_forms(true),
        );
        let receipt = recorder
            .reply(&setup.channel, &setup.event, form(), ReplySource::Tool)
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            "{case}: {:?}",
            receipt.delivery
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(recorder.http.sent().len(), 1);
        let records = setup.backend.records();
        assert_eq!(records.len(), 1);
        let card = setup
            .store
            .load(records.keys().next().unwrap())
            .await
            .unwrap()
            .0;
        assert!(card.destination.is_none());
        assert!(card.reference.is_none());
        recorder.finish().await;
    }
}

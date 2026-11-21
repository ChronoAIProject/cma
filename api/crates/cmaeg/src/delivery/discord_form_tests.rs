use super::{
    discord_controls_tests::sender, gate::recording::RecordingGate, intents::DeliveryOutcome,
    send::SendContext,
};
use crate::discord::{components_fixture::FixtureState, modal_fixture::message};
use cmaeg_core::message::{Block, RenderingOutcome};
use serde_json::{json, Value};

#[tokio::test]
async fn discord_modal_forms_are_default_off_and_native_report_is_honest() {
    for enabled in [false, true] {
        let f = FixtureState::new().await;
        let mut sender = sender(&f, true);
        assert!(!sender.relay.discord_forms_enabled());
        sender.relay = sender.relay.with_discord_forms(enabled);
        f.rest.respond(
            axum::http::StatusCode::OK,
            json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
        );
        if enabled {
            f.rest
                .memory
                .responses
                .lock()
                .unwrap()
                .push_back((axum::http::StatusCode::OK, json!({"id":"99","type":1})));
        }
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message()).await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let calls = f.rest.memory.calls.lock().unwrap();
        let body: Value = serde_json::from_slice(&calls.last().unwrap().body).unwrap();
        assert_eq!(body.get("components").is_some(), enabled);
        assert!(body.get("flags").is_none());
        if enabled {
            assert!(body["components"][0]["components"][0]["custom_id"]
                .as_str()
                .unwrap()
                .starts_with("cdf1:"));
            assert_eq!(receipt.rendering.outcome, RenderingOutcome::Degraded);
            assert!(serde_json::to_string(&receipt.rendering)
                .unwrap()
                .contains("discord_modal_number_date_validated_text"));
            assert!(receipt.message_ref.is_some());
        } else {
            assert!(f.provider.memory.lock().unwrap().seen.is_empty());
        }
    }
}
#[tokio::test]
async fn discord_modal_ineligible_forms_retain_exact_previous_delivery_without_native_state() {
    for case in ["many_fields", "long_label", "long_text", "mixed_controls"] {
        let mut outbound = message();
        match case {
            "many_fields" => {
                let Block::Form { fields, .. } = &mut outbound.blocks[0] else {
                    panic!()
                };
                let f = fields[0].clone();
                *fields = (0..6)
                    .map(|i| {
                        let mut f = f.clone();
                        f.id = format!("field{i}");
                        f
                    })
                    .collect();
            }
            "long_label" => {
                let Block::Form { fields, .. } = &mut outbound.blocks[0] else {
                    panic!()
                };
                fields[0].label = "x".repeat(46);
            }
            "long_text" => outbound.text = Some("x".repeat(2200)),
            _ => outbound
                .blocks
                .extend(crate::discord::components_fixture::message(false).blocks),
        }
        let mut bodies = Vec::new();
        let mut renderings = Vec::new();
        for enabled in [false, true] {
            let f = FixtureState::new().await;
            let mut sender = sender(&f, true);
            sender.relay = sender.relay.with_discord_forms(enabled);
            f.rest.respond(
                axum::http::StatusCode::OK,
                json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
            );
            if case == "mixed_controls" {
                f.rest
                    .memory
                    .responses
                    .lock()
                    .unwrap()
                    .push_back((axum::http::StatusCode::OK, json!({"id":"99","type":1})));
            }
            let before = f.setup.fixture.memory.rows.lock().unwrap().len();
            let gate = RecordingGate::new(true);
            let trace = tracing::Span::none();
            let ctx = SendContext {
                trace: &trace,
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            };
            let (receipt, _) = sender.reply(&ctx, &f.original, &outbound).await;
            assert!(
                matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
                "{case}"
            );
            let requests: Vec<Value> = f
                .rest
                .memory
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.method == "POST")
                .map(|call| {
                    let mut body: Value = serde_json::from_slice(&call.body).unwrap();
                    // Independent fixture event IDs and card UUIDs differ; compare
                    // every user-visible field and the old control kind instead.
                    body.as_object_mut().unwrap().remove("message_reference");
                    if let Some(rows) = body["components"].as_array_mut() {
                        for row in rows {
                            for control in row["components"].as_array_mut().unwrap() {
                                let id = control["custom_id"].as_str().unwrap();
                                assert!(id.starts_with("cd1:"));
                                control["custom_id"] = "legacy-control".into();
                            }
                        }
                    }
                    body
                })
                .collect();
            if case != "mixed_controls" {
                assert_eq!(
                    before,
                    f.setup.fixture.memory.rows.lock().unwrap().len(),
                    "{case}"
                );
                assert!(f.provider.memory.lock().unwrap().seen.is_empty());
            }
            bodies.push(requests);
            renderings.push(serde_json::to_value(receipt.rendering).unwrap());
        }
        assert_eq!(bodies[0], bodies[1], "{case}");
        assert_eq!(renderings[0], renderings[1], "{case}");
    }
}
#[tokio::test]
async fn discord_modal_auth_and_uncertain_delivery_do_not_fallback() {
    for invalid_context in [false, true] {
        let f = FixtureState::new().await;
        let mut sender = sender(&f, true);
        sender.relay = sender.relay.with_discord_forms(true);
        if invalid_context {
            f.provider.memory.lock().unwrap().config.event_context = Some(json!({"invalid":true}));
        } else {
            f.rest
                .memory
                .responses
                .lock()
                .unwrap()
                .push_back((axum::http::StatusCode::OK, json!({"id":"99","type":1})));
        }
        f.rest
            .respond(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({}));
        let before = f.setup.fixture.memory.rows.lock().unwrap().len();
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.original, &message()).await;
        assert!(receipt.message_ref.is_none());
        if invalid_context {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unsupported));
            assert_eq!(before, f.setup.fixture.memory.rows.lock().unwrap().len());
            assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
            let calls = f.rest.memory.calls.lock().unwrap();
            assert_eq!(calls.iter().filter(|call| call.method == "POST").count(), 1);
        }
    }
}

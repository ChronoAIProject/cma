//! Plain edits retain exact platform/route/receipt identity and never infer success.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::*,
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::{
        intents::{DeliveryOutcome, ReplySource},
        relay_client::RelayClient,
    },
    DeliveryConfig,
};
use cmaeg_core::{
    activity::SourceKind,
    channel::{Platform, Source},
    seal::{MessageRefPayload, ReplyAddress},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn fixture(
    platform: Platform,
) -> (
    Arc<cmaeg::channels::resolver::ResolvedChannel>,
    cmaeg_core::seal::EventRefPayload,
) {
    let mut channel = relay_channel(0);
    if let Source::NyxidRelay {
        platform: value, ..
    } = &mut Arc::get_mut(&mut channel).unwrap().definition.sources[0]
    {
        *value = platform;
    }
    let activity = activity(
        SourceKind::NyxidRelay {
            platform,
            route_id: "route-1".into(),
        },
        "msg-1",
    );
    let mut reply = relay_reply();
    if let ReplyAddress::NyxidRelay {
        platform: value, ..
    } = &mut reply
    {
        *value = platform;
    }
    let event = event(&channel, &activity, 1, reply);
    (channel, event)
}

fn recording(complete: bool) -> Recording {
    recording_with_gate(complete, Some(true))
}

fn recording_with_gate(complete: bool, gate: Option<bool>) -> Recording {
    let http = FakeHttp::new(|sent, count| {
        if sent.url.path().ends_with("reply/update") {
            Answer::json(
                200,
                json!({"upstream_message_id":sent.json()["message_id"],"edited_at":"2026-10-26T10:00:00Z"}),
            )
        } else {
            Answer::json(
                200,
                json!({"message_id":format!("out-{count}"),"platform_message_id":format!("platform-{count}")}),
            )
        }
    });
    Recording::start_with_relay(
        http,
        Hooks::answering([]),
        DeliveryConfig::default(),
        None,
        false,
        |relay| {
            let relay = relay.with_complete_replies(complete);
            match gate {
                Some(enabled) => relay.with_text_edits(enabled),
                None => relay,
            }
        },
    )
}

#[tokio::test(start_paused = true)]
async fn ordinary_relay_edits_keep_receipt_route_and_platform_for_every_supported_platform() {
    for complete in [false, true] {
        for platform in [
            Platform::Telegram,
            Platform::Lark,
            Platform::Feishu,
            Platform::Discord,
        ] {
            let (channel, event) = fixture(platform);
            let mut rec = recording(complete);
            let sent = rec
                .reply(
                    &channel,
                    &event,
                    json!({"text":"original"}),
                    ReplySource::Tool,
                )
                .await;
            let reference = sent.message_ref.unwrap().open(&rec.keys).unwrap().payload;
            let destination = reference.relay_message.as_ref().unwrap();
            assert_eq!(destination.platform, platform);
            assert_eq!(destination.outbound_message_id, "out-1");
            let edited = rec
                .update(&channel, reference.clone(), json!({"text":"replacement"}))
                .await;
            assert_eq!(
                edited.delivery,
                DeliveryOutcome::Confirmed {
                    platform_message_id: Some("platform-1".into())
                }
            );
            assert_eq!(
                edited.message_ref.unwrap().open(&rec.keys).unwrap().payload,
                reference
            );
            let calls = rec.http.sent();
            assert_eq!(calls.len(), 2);
            assert_eq!(
                calls[1].json(),
                json!({"message_id":"platform-1","outbound_message_id":"out-1","reply":{"text":"replacement"}})
            );
            assert_eq!(
                calls[1].header("authorization"),
                Some("Bearer recording-channel-key")
            );
            rec.finish().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn edits_refuse_unbound_legacy_or_multipart_references_and_unsupported_replacements_before_io(
) {
    let (channel, event) = fixture(Platform::Discord);
    let mut rec = recording(true);
    let sent = rec
        .reply(
            &channel,
            &event,
            json!({"text":"original"}),
            ReplySource::Tool,
        )
        .await;
    let reference = sent.message_ref.unwrap().open(&rec.keys).unwrap().payload;
    let mut invalid = Vec::<MessageRefPayload>::new();
    let mut legacy = reference.clone();
    legacy.relay_message = None;
    invalid.push(legacy);
    let mut route = reference.clone();
    route.route_id = "other-route".into();
    invalid.push(route);
    let mut wrong_platform = reference.clone();
    wrong_platform.relay_message.as_mut().unwrap().platform = Platform::Lark;
    invalid.push(wrong_platform);
    let mut wrong_channel = reference.clone();
    wrong_channel.channel_id = "cmaeg1.ch.another-channel".parse().unwrap();
    invalid.push(wrong_channel);
    for payload in invalid {
        assert_eq!(
            rec.update(&channel, payload, json!({"text":"replacement"}))
                .await
                .delivery,
            DeliveryOutcome::Unsupported
        );
    }
    assert!(matches!(
        rec.update(
            &channel,
            reference.clone(),
            json!({"text":"x".repeat(2001)})
        )
        .await
        .delivery,
        DeliveryOutcome::Rejected { .. }
    ));
    assert_eq!(
        rec.update(
            &channel,
            reference,
            json!({"text":"replacement", "thread_reply":true})
        )
        .await
        .delivery,
        DeliveryOutcome::Unsupported
    );
    assert_eq!(rec.http.sent().len(), 1);
    let sent = rec
        .reply(
            &channel,
            &event,
            json!({"text":"x".repeat(2001)}),
            ReplySource::Tool,
        )
        .await;
    assert!(sent
        .message_ref
        .unwrap()
        .open(&rec.keys)
        .unwrap()
        .payload
        .relay_message
        .is_none());
    rec.finish().await;
}

#[tokio::test(start_paused = true)]
async fn plain_edit_checks_correlated_receipt_and_does_not_retry_uncertain_responses() {
    for answer in [
        Answer::status(200),
        Answer::status(204),
        Answer::json(
            200,
            json!({"upstream_message_id":"foreign","edited_at":"2026-10-26T10:00:00Z"}),
        ),
        Answer::json(
            200,
            json!({"upstream_message_id":"platform-1","edited_at":"invalid"}),
        ),
        Answer::json(
            200,
            json!({"upstream_message_id":"platform-1","edited_at":"2026-10-26T10:00:00Z"}),
        )
        .after(Duration::from_secs(6)),
    ] {
        let http = Arc::new(FakeHttp::relay().script([answer]));
        let client = RelayClient::new("https://nyx.example", Duration::from_secs(5))
            .unwrap()
            .with_transport(http.clone());
        let result = client
            .edit_text(
                &cmaeg_core::channel::Secret::new(CHANNEL_KEY),
                "platform-1",
                "out-1",
                "replacement",
            )
            .await;
        assert!(matches!(result, DeliveryOutcome::Unknown { .. }));
        assert_eq!(http.sent().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn disabled_or_absent_gate_preserves_legacy_reference_payload_in_both_send_modes() {
    for complete in [false, true] {
        for gate in [None, Some(false)] {
            for platform in [
                Platform::Telegram,
                Platform::Lark,
                Platform::Feishu,
                Platform::Discord,
            ] {
                let (channel, event) = fixture(platform);
                let mut rec = recording_with_gate(complete, gate);
                let sent = rec
                    .reply(
                        &channel,
                        &event,
                        json!({"text":"original"}),
                        ReplySource::Tool,
                    )
                    .await;
                let reference = sent.message_ref.unwrap().open(&rec.keys).unwrap().payload;
                // Sealed bytes are randomized; plaintext must retain the exact legacy shape.
                assert_eq!(
                    serde_json::to_value(reference).unwrap(),
                    json!({
                        "channel_id":event.channel_id, "event_id":event.event_id,
                        "platform_message_id":"platform-1", "route_id":"route-1"
                    })
                );
                assert_eq!(rec.http.sent().len(), 1);
                rec.finish().await;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn disabling_issuance_preserves_edits_of_previously_issued_references() {
    for complete in [false, true] {
        for platform in [
            Platform::Telegram,
            Platform::Lark,
            Platform::Feishu,
            Platform::Discord,
        ] {
            let (channel, event) = fixture(platform);
            let mut issuer = recording(complete);
            let sent = issuer
                .reply(
                    &channel,
                    &event,
                    json!({"text":"original"}),
                    ReplySource::Tool,
                )
                .await;
            let retained = sent.message_ref.unwrap();
            issuer.finish().await;
            let mut reader = recording_with_gate(complete, Some(false));
            let reference = retained.open(&reader.keys).unwrap().payload;
            assert!(reference.relay_message.is_some());
            let edited = reader
                .update(&channel, reference.clone(), json!({"text":"replacement"}))
                .await;
            assert_eq!(
                edited.delivery,
                DeliveryOutcome::Confirmed {
                    platform_message_id: Some("platform-1".into())
                }
            );
            assert_eq!(
                edited
                    .message_ref
                    .unwrap()
                    .open(&reader.keys)
                    .unwrap()
                    .payload,
                reference
            );
            assert_eq!(reader.http.sent().len(), 1);
            reader.finish().await;
        }
    }
}

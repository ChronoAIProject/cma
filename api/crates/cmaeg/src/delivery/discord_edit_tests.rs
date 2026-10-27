use super::{
    discord_tests::setup,
    gate::recording::RecordingGate,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::{channels::resolver::ResolvedChannel, discord::rest_fixture::Fixture};
use axum::http::StatusCode;
use cmaeg_core::{
    channel::{AdmissionPolicy, Secret, Source},
    ids::MessageRef,
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload},
};
use serde_json::{json, Value};
use std::sync::Arc;

#[path = "discord_media_tests.rs"]
mod media;

fn message(text: &str) -> OutboundMessage {
    serde_json::from_value(json!({"text":text})).unwrap()
}
fn response(text: &str) -> Value {
    json!({"id":"800","channel_id":"99","author":{"id":"77"},"content":text})
}
async fn send(
    sender: &Sender,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    message: &OutboundMessage,
) -> IntentReceipt {
    let gate = RecordingGate::new(true);
    let channel = Arc::new(channel.clone());
    sender
        .reply(
            &SendContext {
                trace: &tracing::Span::none(),
                channel: &channel,
                first_send_done: false,
                gate: &gate,
            },
            event,
            message,
        )
        .await
        .0
}
async fn edit(
    sender: &Sender,
    channel: &ResolvedChannel,
    reference: &MessageRefPayload,
    message: &OutboundMessage,
    committed: bool,
) -> IntentReceipt {
    let gate = RecordingGate::new(committed);
    let channel = Arc::new(channel.clone());
    let (receipt, effects) = sender
        .update(
            &SendContext {
                trace: &tracing::Span::none(),
                channel: &channel,
                first_send_done: false,
                gate: &gate,
            },
            reference,
            message,
        )
        .await;
    assert!(!effects.first_send_done);
    receipt
}
async fn issued(f: &Fixture) -> (Sender, Arc<ResolvedChannel>, MessageRefPayload) {
    let (mut sender, channel, event) = setup(f);
    sender.relay = sender.relay.with_native_discord_edits(true);
    let receipt = send(&sender, &channel, &event, &message("original")).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let reference = receipt
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    (sender, channel, reference)
}

#[tokio::test]
async fn native_send_seals_exact_destination_and_patch_retains_reference_after_issuance_disabled() {
    let f = Fixture::new().await;
    let (mut sender, channel, reference) = issued(&f).await;
    let destination = reference.discord_message.as_ref().unwrap();
    assert_eq!(destination.bot_id, "77");
    assert_eq!(destination.channel_id, "99");
    assert_eq!(reference.platform_message_id, "800");
    sender.relay = sender.relay.with_native_discord_edits(false);
    f.respond(StatusCode::OK, response("updated @\u{200b}everyone"));
    let receipt = edit(
        &sender,
        &channel,
        &reference,
        &message("updated @everyone"),
        true,
    )
    .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(
        receipt
            .message_ref
            .unwrap()
            .open(&sender.keys)
            .unwrap()
            .payload,
        reference
    );
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[1].method, "PATCH");
    assert_eq!(calls[1].path, "/api/v10/channels/99/messages/800");
    assert_eq!(
        calls[1].authorization.as_deref(),
        Some("Bot native-local-fixture")
    );
    let body: Value = serde_json::from_slice(&calls[1].body).unwrap();
    assert_eq!(
        body,
        json!({"content":"updated @\u{200b}everyone","allowed_mentions":{"parse":[],"replied_user":false}})
    );
}

#[tokio::test]
async fn verified_thread_reply_and_edit_use_exact_thread_channel_and_never_parent() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    sender.relay = sender.relay.with_native_discord_edits(true);
    let cmaeg_core::seal::ReplyAddress::Discord {
        channel_id,
        thread_id,
        ..
    } = &mut event.reply
    else {
        panic!()
    };
    *channel_id = "100".into();
    *thread_id = Some("100".into());
    f.respond(
        StatusCode::OK,
        json!({"id":"800","channel_id":"100","author":{"id":"77"}}),
    );
    let threaded: OutboundMessage =
        serde_json::from_value(json!({"text":"thread reply","thread_reply":true})).unwrap();
    let receipt = send(&sender, &channel, &event, &threaded).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let reference = receipt
        .message_ref
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    assert_eq!(
        reference.discord_message.as_ref().unwrap().channel_id,
        "100"
    );
    // A reference pins its original thread. An edit cannot add routing fields.
    assert!(matches!(
        edit(&sender, &channel, &reference, &threaded, true)
            .await
            .delivery,
        DeliveryOutcome::Unsupported
    ));
    f.respond(
        StatusCode::OK,
        json!({"id":"800","channel_id":"100","author":{"id":"77"},"content":"updated"}),
    );
    assert!(matches!(
        edit(&sender, &channel, &reference, &message("updated"), true)
            .await
            .delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    {
        let calls = f.memory.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].path, "/api/v10/channels/100/messages");
        let sent: Value = serde_json::from_slice(&calls[0].body).unwrap();
        assert_eq!(sent["message_reference"]["message_id"], "700");
        assert_eq!(calls[1].path, "/api/v10/channels/100/messages/800");
    }
    for proof in [None, Some("99".into())] {
        let cmaeg_core::seal::ReplyAddress::Discord { thread_id, .. } = &mut event.reply else {
            panic!()
        };
        *thread_id = proof;
        assert!(matches!(
            send(&sender, &channel, &event, &threaded).await.delivery,
            DeliveryOutcome::Unsupported
        ));
    }
    assert_eq!(f.memory.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_edit_authority_refuses_revoked_source_owner_consumer_key_or_record_before_io() {
    let f = Fixture::new().await;
    let (sender, channel, reference) = issued(&f).await;
    for variant in 0..9 {
        let mut current = (*channel).clone();
        match variant {
            0 => current.definition.sources.clear(),
            1 => current.definition.secrets.discord_bot_token = None,
            2 => current.definition.owner.subject.push_str("-changed"),
            3 => current.agent_key_hash = [8; 32],
            4 => current.pointer.record_id = "other-record".parse().unwrap(),
            5 => current.id = "cmaeg1.ch.other".parse().unwrap(),
            6 => {
                current.definition.sources = vec![Source::Discord {
                    thread_history: false,
                    thread_lifecycle: false,
                    bot_id: "88".into(),
                    intents: 37377,
                    admission: AdmissionPolicy::All {},
                }]
            }
            7 => {
                if let Source::Discord { intents, .. } = &mut current.definition.sources[0] {
                    *intents ^= 1;
                }
            }
            _ => {
                let mut definition = current.definition.to_blob_json();
                definition["provider"]["binding_id"] = "other-binding".into();
                current.definition = serde_json::from_value(definition).unwrap();
            }
        }
        let result = edit(&sender, &current, &reference, &message("replacement"), true).await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unsupported),
            "variant {variant}"
        );
        assert!(result.message_ref.is_none());
    }
    assert_eq!(f.memory.calls.lock().unwrap().len(), 1);
    assert_eq!(f.memory.identities.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn token_rotation_requires_actual_current_bot_identity_and_preserves_same_bot_authority() {
    let f = Fixture::new().await;
    let (sender, channel, reference) = issued(&f).await;
    let mut current = (*channel).clone();
    current.definition.secrets.discord_bot_token = Some(Secret::new("native-rotated-fixture"));
    f.respond(StatusCode::OK, response("replacement"));
    let result = edit(&sender, &current, &reference, &message("replacement"), true).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!(
        f.memory.calls.lock().unwrap()[1].authorization.as_deref(),
        Some("Bot native-rotated-fixture")
    );
    assert_eq!(f.memory.identities.lock().unwrap().len(), 2);
    current.definition.secrets.discord_bot_token = Some(Secret::new("native-other-fixture"));
    *f.memory.bot.lock().unwrap() = json!({"id":"88","bot":true});
    let result = edit(&sender, &current, &reference, &message("replacement"), true).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Rejected { .. }));
    assert_eq!(f.memory.calls.lock().unwrap().len(), 2);
    assert_eq!(f.memory.identities.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn edit_bounds_and_denied_commit_do_not_write_and_references_cannot_be_retargeted() {
    let f = Fixture::new().await;
    let (sender, channel, reference) = issued(&f).await;
    let cases = [
        json!({"text":"x","native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="}}]}),
        json!({"text":"x","thread_reply":true}),
        json!({"text":"x","blocks":[{"type":"card","title":"card","fields":[]}]}),
        json!({"text":"😀".repeat(1001)}),
    ];
    for value in cases {
        let result = edit(
            &sender,
            &channel,
            &reference,
            &serde_json::from_value(value).unwrap(),
            true,
        )
        .await;
        assert!(!matches!(
            result.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(result.message_ref.is_none());
    }
    let result = edit(
        &sender,
        &channel,
        &reference,
        &message("replacement"),
        false,
    )
    .await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Rejected { reason } if reason=="delivery_expired")
    );
    let mut wrong = reference.clone();
    wrong.discord_message.as_mut().unwrap().channel_id = "100".into();
    assert!(MessageRef::seal(&wrong, &sender.keys)
        .unwrap()
        .open(&sender.keys)
        .is_err());
    assert!(matches!(
        edit(&sender, &channel, &wrong, &message("replacement"), true)
            .await
            .delivery,
        DeliveryOutcome::Unsupported
    ));
    assert_eq!(f.memory.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn disabled_media_multipart_and_uncertain_sends_never_issue_edit_authority() {
    let f = Fixture::new().await;
    let (mut sender, channel, event) = setup(&f);
    assert!(send(&sender, &channel, &event, &message("original"))
        .await
        .message_ref
        .is_none());
    sender.relay = sender.relay.with_native_discord_edits(true);
    for value in [
        json!({"text":"😀".repeat(1001)}),
        json!({"text":"file","native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="}}]}),
        json!({"blocks":[{"type":"card","title":"card","fields":[]}]}),
    ] {
        let result = send(
            &sender,
            &channel,
            &event,
            &serde_json::from_value(value).unwrap(),
        )
        .await;
        assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
        assert!(result.message_ref.is_none());
    }
    f.memory.responses.lock().unwrap().extend([
        (StatusCode::OK, response("first")),
        (StatusCode::BAD_GATEWAY, json!({})),
    ]);
    let partial = send(&sender, &channel, &event, &message(&"x".repeat(2001))).await;
    assert!(
        matches!(partial.delivery, DeliveryOutcome::Unknown { reason } if reason=="partial_delivery")
    );
    assert!(partial.message_ref.is_none());
    *f.memory.broken.lock().unwrap() = true;
    let lost = send(&sender, &channel, &event, &message("lost")).await;
    assert!(matches!(lost.delivery, DeliveryOutcome::Unknown { .. }));
    assert!(lost.message_ref.is_none());
    assert_eq!(f.memory.calls.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn native_edit_rate_limits_and_response_loss_never_retry_or_claim_success() {
    let f = Fixture::new().await;
    let (sender, channel, reference) = issued(&f).await;
    *f.memory.broken.lock().unwrap() = true;
    let result = edit(&sender, &channel, &reference, &message("replacement"), true).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
    assert!(result.message_ref.is_none());
    assert_eq!(f.memory.calls.lock().unwrap().len(), 2);
    *f.memory.broken.lock().unwrap() = false;
    for value in [
        json!({}),
        response("wrong text"),
        json!({"id":"801","channel_id":"99","author":{"id":"77"},"content":"replacement"}),
    ] {
        f.respond(StatusCode::OK, value);
        let result = edit(&sender, &channel, &reference, &message("replacement"), true).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(result.message_ref.is_none());
    }
    f.respond(StatusCode::TOO_MANY_REQUESTS, json!({"retry_after":1}));
    let result = edit(&sender, &channel, &reference, &message("replacement"), true).await;
    assert!(matches!(
        result.delivery,
        DeliveryOutcome::RateLimited { .. }
    ));
    assert!(result.message_ref.is_none());
    assert_eq!(f.memory.calls.lock().unwrap().len(), 6);
}

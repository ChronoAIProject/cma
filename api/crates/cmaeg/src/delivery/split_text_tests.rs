use super::{
    gate::recording::RecordingGate,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::channels::resolver::ResolvedChannel;
use axum::http::StatusCode;
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress, TelegramAdmissionProof},
};
use serde_json::json;

async fn send(
    sender: &Sender,
    channel: &std::sync::Arc<ResolvedChannel>,
    event: &EventRefPayload,
    text: &str,
) -> IntentReceipt {
    sender
        .reply(
            &SendContext {
                trace: &tracing::Span::none(),
                channel,
                first_send_done: false,
                gate: &RecordingGate::new(true),
            },
            event,
            &serde_json::from_value(json!({"text":text})).unwrap(),
        )
        .await
        .0
}

#[tokio::test]
async fn telegram_partial_split_retains_exact_confirmed_parts_and_legacy_first_reference() {
    use crate::telegram::media_fixture::{Answer, Fixture};
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = super::telegram_media_tests::setup(&f);
    sender.relay = sender
        .relay
        .with_native_split_text_refs(true)
        .with_telegram_effects(true);
    event.activity_sha256 = Some("a".repeat(64));
    event.telegram_admission = Some(TelegramAdmissionProof {
        actor_id: "42".into(),
        actor_kind: cmaeg_core::activity::ActorKind::Human,
        conversation_kind: cmaeg_core::activity::ConversationKind::Group,
        mentioned: Some(true),
        replied_to_bot: Some(false),
    });
    for id in [800, 801] {
        f.answer(Answer::json(
            StatusCode::OK,
            json!({"ok":true,"result":{"message_id":id,"chat":{"id":-42},"message_thread_id":7}}),
        ));
    }
    f.answer(Answer::json(StatusCode::BAD_GATEWAY, json!({})));
    let result = send(&sender, &channel, &event, &"x".repeat(8001)).await;
    assert!(
        matches!(result.delivery, DeliveryOutcome::Unknown { ref reason } if reason == "partial_delivery")
    );
    let first = result
        .message_ref
        .as_ref()
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    assert_eq!(first.platform_message_id, "800");
    assert!(
        first.telegram_effect.is_none(),
        "legacy partial enrichment is unchanged"
    );
    assert_eq!(result.message_parts.len(), 2);
    for (index, part) in result.message_parts.iter().enumerate() {
        let reference = part.message_ref.open(&sender.keys).unwrap().payload;
        assert_eq!(usize::from(part.index), index);
        assert_eq!(reference.platform_message_id, (800 + index).to_string());
        assert_eq!(reference.channel_id, event.channel_id);
        assert_eq!(reference.event_id, event.event_id);
        assert_eq!(reference.consumer, event.consumer);
        assert_eq!(reference.route_id, "telegram:77:-42");
        assert_eq!(
            reference.telegram_effect.unwrap().thread_id.as_deref(),
            Some("7")
        );
    }
    assert_eq!(f.calls().len(), 3);
}

#[tokio::test]
async fn telegram_split_parts_require_exact_chat_topic_and_individual_message_receipts() {
    use crate::telegram::media_fixture::{Answer, Fixture};
    for invalid in [
        json!({"message_id":801,"chat":{"id":-43},"message_thread_id":7}),
        json!({"message_id":801,"chat":{"id":-42},"message_thread_id":8}),
        json!({"message_id":801,"chat":{"id":-42}}),
        json!({"message_id":801,"chat":{"id":-42},"message_thread_id":7,"media_group_id":"album"}),
        json!({"message_id":0,"chat":{"id":-42},"message_thread_id":7}),
    ] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = super::telegram_media_tests::setup(&f);
        sender.relay = sender.relay.with_native_split_text_refs(true);
        f.answer(Answer::ok());
        f.answer(Answer::json(
            StatusCode::OK,
            json!({"ok":true,"result":invalid}),
        ));
        let result = send(&sender, &channel, &event, &"x".repeat(4001)).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        assert_eq!(result.message_parts.len(), 1);
        assert_eq!(result.message_parts[0].index, 0);
        assert_eq!(
            result.message_parts[0]
                .message_ref
                .open(&sender.keys)
                .unwrap()
                .payload
                .platform_message_id,
            "800"
        );
        assert_eq!(f.calls().len(), 2);
    }
}

#[tokio::test]
async fn telegram_split_rate_limit_retry_checks_topic_but_gate_off_preserves_legacy_receipts() {
    use crate::telegram::media_fixture::{Answer, Fixture};
    for split in [false, true] {
        let f = Fixture::new().await;
        let (mut sender, channel, event) = super::telegram_media_tests::setup(&f);
        sender.relay = sender.relay.with_native_split_text_refs(split);
        f.answer(Answer::json(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"ok":false,"error_code":429,"parameters":{"retry_after":1}}),
        ));
        f.answer(Answer::json(
            StatusCode::OK,
            json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42}}}),
        ));
        let result = send(&sender, &channel, &event, &"x".repeat(4001)).await;
        assert!(result.message_parts.is_empty());
        if split {
            assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(result.message_ref.is_none());
            assert_eq!(f.calls().len(), 2);
        } else {
            assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
            assert!(result.message_ref.is_some());
            assert_eq!(f.calls().len(), 3);
        }
    }
}

#[tokio::test]
async fn discord_split_references_edit_exact_part_and_remain_scoped_when_issuance_disabled() {
    let f = crate::discord::rest_fixture::Fixture::new().await;
    let (mut sender, mut channel, mut event) = super::discord_tests::setup(&f);
    let routed =
        crate::channels::routing_fixture::definition(crate::channels::routing_fixture::routed());
    let reviewer =
        cmaeg_core::ids::ConsumerId::parse(crate::channels::routing_fixture::REVIEWER).unwrap();
    let current = std::sync::Arc::get_mut(&mut channel).unwrap();
    current.definition.routing = routed.routing;
    current.definition.secrets.consumer_keys = routed.secrets.consumer_keys;
    current.consumer_keys = vec![(reviewer.clone(), [9; 32])];
    event.consumer = Some(reviewer);
    sender.relay = sender
        .relay
        .with_native_split_text_refs(true)
        .with_native_discord_edits(true)
        .with_native_discord_effects(true);
    if let ReplyAddress::Discord {
        channel_id,
        thread_id,
        ..
    } = &mut event.reply
    {
        *channel_id = "100".into();
        *thread_id = Some("100".into());
    }
    for id in ["800", "801"] {
        f.memory.responses.lock().unwrap().push_back((
            StatusCode::OK,
            json!({"id":id,"channel_id":"100","author":{"id":"77"}}),
        ));
    }
    let result = send(&sender, &channel, &event, &"😀".repeat(1001)).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert!(
        result.message_ref.is_none(),
        "legacy multipart semantics stay unchanged"
    );
    assert_eq!(result.message_parts.len(), 2);
    let references: Vec<_> = result
        .message_parts
        .iter()
        .map(|part| part.message_ref.open(&sender.keys).unwrap().payload)
        .collect();
    assert_eq!(references[0].platform_message_id, "800");
    assert_eq!(references[1].platform_message_id, "801");
    for reference in &references {
        assert_eq!(
            reference.discord_message.as_ref().unwrap().channel_id,
            "100"
        );
        assert_eq!(reference.consumer, event.consumer);
        assert_eq!(reference.event_id, event.event_id);
    }
    sender.relay = sender
        .relay
        .with_native_split_text_refs(false)
        .with_native_discord_edits(false);
    f.respond(
        StatusCode::OK,
        json!({"id":"801","channel_id":"100","author":{"id":"77"},"content":"updated"}),
    );
    let message: OutboundMessage = serde_json::from_value(json!({"text":"updated"})).unwrap();
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &channel,
        first_send_done: false,
        gate: &RecordingGate::new(true),
    };
    assert!(matches!(
        sender
            .update(&ctx, &references[1], &message)
            .await
            .0
            .delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(
        f.memory.calls.lock().unwrap()[2].path,
        "/api/v10/channels/100/messages/801"
    );
    let mut revoked = (*channel).clone();
    revoked.consumer_keys[0].1 = [8; 32];
    let revoked = std::sync::Arc::new(revoked);
    let ctx = SendContext {
        channel: &revoked,
        ..ctx
    };
    assert!(matches!(
        sender
            .update(&ctx, &references[1], &message)
            .await
            .0
            .delivery,
        DeliveryOutcome::Unsupported
    ));
    assert_eq!(f.memory.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn discord_partial_and_unknown_sends_never_reference_unconfirmed_parts() {
    for confirmed in [false, true] {
        let f = crate::discord::rest_fixture::Fixture::new().await;
        let (mut sender, channel, event) = super::discord_tests::setup(&f);
        sender.relay = sender
            .relay
            .with_native_split_text_refs(true)
            .with_native_discord_edits(true);
        if confirmed {
            f.memory.responses.lock().unwrap().push_back((
                StatusCode::OK,
                json!({"id":"800","channel_id":"99","author":{"id":"77"}}),
            ));
        }
        f.respond(StatusCode::BAD_GATEWAY, json!({}));
        let result = send(&sender, &channel, &event, &"x".repeat(2001)).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(result.message_ref.is_none());
        assert_eq!(result.message_parts.len(), usize::from(confirmed));
        assert_eq!(
            f.memory.calls.lock().unwrap().len(),
            1 + usize::from(confirmed)
        );
    }
}

#[tokio::test]
async fn split_issuance_is_default_off_requires_existing_discord_edit_gate_and_omits_single_parts()
{
    for (split, edits, length) in [(false, true, 2001), (true, false, 2001), (true, true, 10)] {
        let f = crate::discord::rest_fixture::Fixture::new().await;
        let (mut sender, channel, event) = super::discord_tests::setup(&f);
        assert!(!sender.relay.native_split_text_refs);
        sender.relay = sender
            .relay
            .with_native_split_text_refs(split)
            .with_native_discord_edits(edits);
        let result = send(&sender, &channel, &event, &"x".repeat(length)).await;
        assert!(result.message_parts.is_empty());
        assert!(serde_json::to_value(&result)
            .unwrap()
            .get("message_parts")
            .is_none());
    }
}

#[tokio::test]
async fn event_side_message_replays_confirmed_telegram_prefix_after_its_second_part_times_out() {
    use super::{
        gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate, Ungated},
        side_messages::SideMessageDelivery,
    };
    use crate::{
        ingress::routing_store::test_server::Server,
        telegram::media_fixture::{Answer, Fixture},
        tools::{reply_targets::receipts, side_messages::claim_side_message},
    };
    use futures_util::future::BoxFuture;
    use std::time::Duration;

    struct Deadline(tokio::time::Instant);
    impl SendGate for Deadline {
        fn enter_route<'a>(
            &'a self,
            route: &'a str,
        ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
            Ungated.enter_route(route)
        }
        fn commit(&self) -> bool {
            true
        }
        fn attempt(&self) -> BoxFuture<'_, Result<AttemptPermit, GateRefusal>> {
            Ungated.attempt()
        }
        fn progress(&self) {}
        fn transport_deadline(&self) -> Option<tokio::time::Instant> {
            Some(self.0)
        }
    }

    let platform = Fixture::new().await;
    let (mut sender, channel, mut event) = super::telegram_media_tests::setup(&platform);
    sender.relay = sender.relay.with_native_split_text_refs(true);
    platform.answer(Answer::ok());
    platform.answer(Answer {
        delay: Duration::from_secs(12),
        ..Answer::ok()
    });

    let backend = Server::start_realtime().await;
    event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 900;
    let store = receipts::Store::new(backend.records.clone(), sender.keys.clone());
    let owner = &channel.definition.owner.subject;
    let message = serde_json::from_value(json!({"text":"x".repeat(4001)})).unwrap();
    let receipts::Admission::Claimed(claim) =
        claim_side_message(&store, &event, "split-timeout", &message, owner)
            .await
            .unwrap()
    else {
        panic!("the initial side message must reserve its receipt");
    };
    let delivery = SideMessageDelivery::new(&sender);
    let started = tokio::time::Instant::now();
    let receipt = delivery
        .send(
            &channel,
            &event,
            &message,
            &Deadline(started + Duration::from_secs(4)),
        )
        .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the second part must use the side-send deadline"
    );
    assert_eq!(receipt.message_parts.len(), 1);
    assert_eq!(
        receipt.message_parts[0]
            .message_ref
            .open(&sender.keys)
            .unwrap()
            .payload
            .platform_message_id,
        "800"
    );
    store.finish(claim, &receipt).await.unwrap();
    let receipts::Admission::Replay(replay) =
        claim_side_message(&store, &event, "split-timeout", &message, owner)
            .await
            .unwrap()
    else {
        panic!("the exact retry must replay its retained receipt");
    };
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
    assert_eq!(platform.calls().len(), 2);
}

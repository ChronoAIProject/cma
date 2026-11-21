use super::*;
use crate::{
    delivery::{custom::CustomDelivery, sync::SyncRegistry},
    turns::routed::{
        delivery_tests::accepted, test_support::*, CustomCheckpoint, DeliveryStatus, ReplayEffect,
    },
};
use axum::{
    body::to_bytes,
    response::{IntoResponse, Sse},
};
use std::sync::Arc;

#[tokio::test]
async fn separate_waiter_registries_preserve_order_and_reject_repeat_or_disconnected_delivery() {
    let origin = Arc::new(SyncRegistry::default().with_instance("a".repeat(64)));
    let worker = Arc::new(SyncRegistry::default().with_instance("b".repeat(64)));
    let (id, waiter) = origin.register().unwrap();
    let request = fixture::request(fixture::targets, fixture::custom(None));
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    event.reply = ReplyAddress::Sync { waiter: id.clone() };
    assert_eq!(waiter_owner(&event), Some("a".repeat(64).as_str()));
    let remote = CustomDelivery::new("https://gateway.example".into(), worker);
    let local = CustomDelivery::new("https://gateway.example".into(), origin.clone());
    let first = CustomStreamEffect::Delta {
        text: "first".into(),
    };
    // The original defect: this frame cannot enter A's waiter from B's registry.
    assert_eq!(
        remote.local(&event, &first),
        LocalStreamOutcome::Unavailable
    );
    assert!(origin.claim_relay(&id, 2));
    assert_eq!(local.local(&event, &first), LocalStreamOutcome::Buffered);
    assert!(!origin.claim_relay(&id, 2));
    assert!(!origin.claim_relay(&id, 1));
    assert!(origin.claim_relay(&id, 5));
    let last = CustomStreamEffect::Completed {
        final_text: Some("last".into()),
    };
    assert_eq!(local.local(&event, &last), LocalStreamOutcome::Buffered);
    assert!(!origin.claim_relay(&id, 5));
    let text = to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 10000)
        .await
        .unwrap();
    let text = std::str::from_utf8(&text).unwrap();
    assert_eq!(text.matches("event: cmaeg.text.delta").count(), 1);
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1);
    assert!(text.find("first").unwrap() < text.find("last").unwrap());
    let (closed, waiter) = origin.register().unwrap();
    drop(waiter);
    assert!(!origin.claim_relay(&closed, 0));
}

#[test]
fn stream_envelopes_cannot_be_replayed_as_tool_calls_or_responses() {
    let keys = fixture::keys();
    let id = uuid::Uuid::new_v4().to_string();
    let envelope = codec::seal(
        &keys,
        Direction::StreamRequest,
        &id,
        CONTEXT,
        fixture::NOW + 5,
        fixture::NOW,
        &42u64,
    )
    .unwrap();
    assert_eq!(
        codec::open::<u64>(
            &keys,
            Direction::StreamRequest,
            CONTEXT,
            fixture::NOW,
            &envelope
        )
        .unwrap(),
        42
    );
    for direction in [
        Direction::Request,
        Direction::Response,
        Direction::StreamResponse,
    ] {
        assert!(codec::open::<u64>(&keys, direction, CONTEXT, fixture::NOW, &envelope).is_err());
    }
    assert!(codec::open::<u64>(
        &keys,
        Direction::StreamRequest,
        "other",
        fixture::NOW,
        &envelope
    )
    .is_err());
    assert!(codec::open::<u64>(
        &keys,
        Direction::StreamRequest,
        CONTEXT,
        fixture::NOW + 5,
        &envelope
    )
    .is_err());
}

#[tokio::test]
async fn stream_requires_exact_live_pending_checkpoint_and_effect() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let effect = CustomStreamEffect::Completed {
        final_text: Some("answer".into()),
    };
    let checkpoint = DeliveryCheckpoint {
        execution_attribution: None,
        trace_contract: false,
        response_id: "resp_1".into(),
        cursor: 5,
        frame_hash: "a".repeat(64),
        effect_hash: storage::hash(&storage::bytes(&ReplayEffect::Custom(effect.clone())).unwrap()),
        terminal: true,
        status: DeliveryStatus::Intent,
        custom: Some(CustomCheckpoint {
            local: Some(LocalStreamOutcome::Pending),
            webhooks: vec![],
        }),
    };
    lease.state.delivery = Some(checkpoint.clone());
    let request = Request {
        identity: setup.plan.identity.clone(),
        target: setup.target().id.clone(),
        ownership: lease.state.ownership.clone(),
        partition: PartitionFence {
            instance: "b".repeat(64),
            epoch: 1,
            claim: uuid::Uuid::new_v4().to_string(),
            until_ms: (fixture::NOW + 10) * 1000,
        },
        checkpoint,
        effect,
        channel_version: 1,
    };
    assert!(receive::validate(&lease.state, &request, &setup.plan, fixture::NOW).is_ok());
    for case in 0..8 {
        let mut state = lease.state.clone();
        match case {
            0 => state.ownership.epoch += 1,
            1 => state.ownership.until = fixture::NOW,
            2 => state.event_expires_at = fixture::NOW,
            3 => state.root.push('x'),
            4 => state.delivery.as_mut().unwrap().cursor += 1,
            5 => {
                state.delivery.as_mut().unwrap().status = DeliveryStatus::Settled { receipt: None }
            }
            6 => {
                state
                    .delivery
                    .as_mut()
                    .unwrap()
                    .custom
                    .as_mut()
                    .unwrap()
                    .local = Some(LocalStreamOutcome::Lost)
            }
            _ => state.delivery.as_mut().unwrap().effect_hash = "b".repeat(64),
        }
        assert!(
            receive::validate(&state, &request, &setup.plan, fixture::NOW).is_err(),
            "case {case}"
        );
    }
    let mut request = request;
    request.effect = CustomStreamEffect::Completed {
        final_text: Some("changed".into()),
    };
    assert!(receive::validate(&lease.state, &request, &setup.plan, fixture::NOW).is_err());
}

#[test]
fn stream_envelope_preserves_full_admitted_effect_size_without_expanding_tool_limits() {
    let keys = fixture::keys();
    let id = uuid::Uuid::new_v4().to_string();
    let text = "a".repeat(280 * 1024);
    let envelope = codec::seal(
        &keys,
        Direction::StreamRequest,
        &id,
        CONTEXT,
        fixture::NOW + 5,
        fixture::NOW,
        &text,
    )
    .unwrap();
    assert!(storage::bytes(&envelope).unwrap().len() < codec::STREAM_BODY_LIMIT);
    assert_eq!(
        codec::open::<String>(
            &keys,
            Direction::StreamRequest,
            CONTEXT,
            fixture::NOW,
            &envelope
        )
        .unwrap(),
        text
    );
    assert!(codec::seal(
        &keys,
        Direction::Request,
        &id,
        CONTEXT,
        fixture::NOW + 5,
        fixture::NOW,
        &text
    )
    .is_err());
}

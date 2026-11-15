use super::{
    delivery_tests::{accepted, lane},
    projection_store_tests::{delta, frame},
    replay_projection::ReplayProjection,
    test_support::*,
    *,
};
use crate::providers::ResponsesEvent;
use crate::telemetry::turn_trace::Association;
use serde_json::{json, Value};

fn annotation() -> Value {
    json!({"version":1,"turn_id":"11111111-1111-4111-8111-111111111111",
        "trace_id":"11111111111141118111111111111111","parent_span_id":"0123456789abcdef",
        "ingress_request_id":"22222222-2222-4222-8222-222222222222"})
}

#[tokio::test]
async fn canonical_turn_survives_encrypted_projection_restart_without_additional_writes() {
    use std::sync::atomic::Ordering;
    let first = Association::parse(&annotation()).unwrap();
    let mut newer = annotation();
    newer["turn_id"] = "44444444-4444-4444-8444-444444444444".into();
    newer["trace_id"] = "44444444444444448444444444444444".into();
    let newer = Association::parse(&newer).unwrap();
    let mut writes = Vec::new();
    for negotiated in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let frozen_bytes = body.provider_body();
        let before = setup.server.memory.writes.load(Ordering::SeqCst);
        let mut projection = ReplayProjection::new("resp_1".into());
        let legacy = serde_json::to_value(&projection).unwrap();
        assert!(legacy.get("trace").is_none());
        let mut initial = delta(0, "first");
        initial.trace_contract = negotiated;
        initial.trace = Some(first);
        projection
            .project(&initial, &body, false, "failed")
            .unwrap();
        setup
            .journal()
            .save_projection(&mut lease, &projection)
            .await
            .unwrap();
        let mut restored = setup
            .journal()
            .load_projection(&lease)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.trace.get(), negotiated.then_some(first));
        // The next authenticated provider frame can describe a conflicting
        // attempt, but execution continues with the original diagnostic turn.
        let mut next = delta(1, "second");
        next.trace_contract = negotiated;
        next.trace = Some(newer);
        let (delivery, _sends) = lane(&setup);
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &initial,
                ReplayEffect::NoEffect,
            )
            .await
            .unwrap();
        restored.project(&next, &body, false, "failed").unwrap();
        setup
            .journal()
            .save_projection(&mut lease, &restored)
            .await
            .unwrap();
        let restored = setup
            .journal()
            .load_projection(&lease)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.trace.get(), negotiated.then_some(first));
        assert_eq!(
            setup.journal().body(&lease).await.unwrap().provider_body(),
            frozen_bytes
        );
        writes.push(setup.server.memory.writes.load(Ordering::SeqCst) - before);
        for row in setup.server.memory.rows.lock().unwrap().values() {
            assert!(!row
                .blob
                .windows(36)
                .any(|bytes| bytes == first.turn_id.to_string().as_bytes()));
        }
        // Invalid annotations in an otherwise authentic checkpoint are gaps,
        // never execution corruption or retained provider-controlled strings.
        let mut value = serde_json::to_value(&restored).unwrap();
        value["trace"] = json!({"private-retained-trace-canary":"invalid"});
        let gap: ReplayProjection = serde_json::from_value(value).unwrap();
        gap.validate(&lease).unwrap();
        assert!(gap.trace.is_empty());
        assert!(!serde_json::to_string(&gap)
            .unwrap()
            .contains("private-retained"));
    }
    assert_eq!(writes[0], writes[1]);
}

#[tokio::test]
async fn trace_attempt_ids_cannot_corrupt_a_retained_projection_or_resend_a_frame() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let frozen_bytes = body.provider_body();
    let mut projection = ReplayProjection::new("resp_1".into());
    let mut original = delta(0, "once");
    original.trace_contract = true;
    original.raw["nyxid_trace"] = annotation();
    let effect = projection
        .project(&original, &body, false, "failed")
        .unwrap();
    setup
        .journal()
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    let (delivery, mut sends) = lane(&setup);
    delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &original,
            effect,
        )
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    let mut lease = accepted(&setup).await;
    let projection = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let mut replay = original.clone();
    replay.raw["nyxid_trace"]["parent_span_id"] = "fedcba9876543210".into();
    replay.raw["nyxid_trace"]["ingress_request_id"] = "33333333-3333-4333-8333-333333333333".into();
    // Recovery after disabling the writer still recognizes its stored digest.
    replay.trace_contract = false;
    let effect = projection.pending_effect(&replay).unwrap().unwrap();
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &replay,
                effect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    assert!(sends.try_recv().is_err());
    assert_eq!(
        setup.journal().body(&lease).await.unwrap().provider_body(),
        frozen_bytes
    );

    for (field, value) in [
        ("delta", json!("changed")),
        ("response_id", json!("foreign")),
        ("metadata", json!({"nyxid_trace":"execution-field-canary"})),
    ] {
        let mut changed = replay.clone();
        changed.raw[field] = value;
        assert!(matches!(
            projection.pending_effect(&changed),
            Err(ExecutionError::Corrupt)
        ));
    }
}

#[tokio::test]
async fn trace_metadata_events_checkpoint_without_becoming_model_output() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    let mut event = frame(
        json!({"type":"response.nyxid_trace","sequence_number":0,"nyxid_trace":annotation()}),
    );
    event.trace_contract = true;
    event.event = ResponsesEvent::NyxidTrace;
    let effect = projection.project(&event, &body, false, "failed").unwrap();
    assert!(matches!(effect, ReplayEffect::NoEffect));
    setup
        .journal()
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    let (delivery, mut sends) = lane(&setup);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &event,
                effect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { cursor: 0, .. }
    ));
    assert!(sends.try_recv().is_err());
    // Even malformed diagnostic replacement cannot retarget execution.
    event.raw["nyxid_trace"] = json!({"private-canary":"invalid"});
    assert!(matches!(
        projection.pending_effect(&event).unwrap(),
        Some(ReplayEffect::NoEffect)
    ));
}

#[tokio::test]
async fn writer_rollback_keeps_reserved_annotations_out_of_later_frame_integrity() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    let mut original = delta(0, "first");
    original.trace_contract = true;
    projection
        .project(&original, &body, false, "failed")
        .unwrap();
    let (delivery, _) = lane(&setup);
    delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &original,
            ReplayEffect::NoEffect,
        )
        .await
        .unwrap();
    setup
        .journal()
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    let mut restored = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let mut after_rollback = delta(1, "second");
    after_rollback.raw["nyxid_trace"] = annotation();
    assert!(!after_rollback.trace_contract);
    restored.retain_trace_contract(&mut after_rollback);
    assert!(after_rollback.trace_contract);
    restored
        .project(&after_rollback, &body, false, "failed")
        .unwrap();
    let encoded = serde_json::to_value(&restored).unwrap();
    let restored: ReplayProjection = serde_json::from_value(encoded).unwrap();
    let mut retry = after_rollback.clone();
    retry.trace_contract = false;
    retry.raw["nyxid_trace"] = json!({"private-new-attempt-canary":"invalid"});
    assert!(matches!(
        restored.pending_effect(&retry).unwrap(),
        Some(ReplayEffect::NoEffect)
    ));
    retry.raw["delta"] = json!("changed execution content");
    assert!(matches!(
        restored.pending_effect(&retry),
        Err(ExecutionError::Corrupt)
    ));
}

#[tokio::test]
async fn trace_hash_ignores_only_reserved_annotations_and_legacy_mode_keeps_exact_hash() {
    let setup = Setup::new(supported()).await;
    let lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let original = frame(
        json!({"type":"response.completed","sequence_number":0,"response":{"id":"resp_1","output":[]}}),
    );
    let legacy_hash = storage::hash(&storage::bytes(&original.raw).unwrap());
    assert_eq!(replay_frame::hash(&original, false).unwrap(), legacy_hash);
    let mut projection = ReplayProjection::new("resp_1".into());
    projection
        .project(&original, &body, false, "failed")
        .unwrap();
    assert!(!serde_json::to_string(&projection)
        .unwrap()
        .contains("trace_contract"));
    let mut annotated = original.clone();
    annotated.raw["nyxid_trace"] = annotation();
    annotated.raw["response"]["nyxid_trace"] = annotation();
    assert_eq!(replay_frame::hash(&annotated, true).unwrap(), legacy_hash);
    assert_ne!(replay_frame::hash(&annotated, false).unwrap(), legacy_hash);
    annotated.raw["response"]["output"] = json!([{"type":"message","content":[{"type":"output_text","text":"nyxid_trace must stay content"}]}]);
    assert_ne!(replay_frame::hash(&annotated, true).unwrap(), legacy_hash);
}

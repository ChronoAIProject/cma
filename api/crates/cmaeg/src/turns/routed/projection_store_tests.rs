use super::{
    delivery_tests::{accepted, lane},
    replay_projection::ReplayProjection,
    test_support::*,
    *,
};
use crate::providers::{parse_event, RecoveryEvent, SseFrame};
use serde_json::{json, Value};

pub(super) fn frame(mut raw: Value) -> RecoveryEvent {
    if raw["type"]
        .as_str()
        .is_some_and(|v| v.starts_with("response.output_item."))
    {
        raw["output_index"] = json!(0);
    }
    raw["response_id"] = json!("resp_1");
    RecoveryEvent {
        trace_contract: false,
        trace: None,
        response_id: "resp_1".into(),
        sequence_number: raw["sequence_number"].as_u64().unwrap(),
        event: parse_event(&SseFrame {
            event: None,
            id: None,
            data: raw.to_string(),
            retry: None,
        })
        .unwrap(),
        raw,
    }
}
pub(super) fn delta(sequence: u64, text: &str) -> RecoveryEvent {
    frame(
        json!({"type":"response.output_text.delta","sequence_number":sequence,"item_id":"message","delta":text}),
    )
}
async fn apply(
    setup: &Setup,
    lease: &mut Lease,
    projection: &mut ReplayProjection,
    frame: &RecoveryEvent,
) {
    let body = setup.journal().body(lease).await.unwrap();
    let effect = projection.project(frame, &body, false, "failed").unwrap();
    setup
        .journal()
        .save_projection(lease, projection)
        .await
        .unwrap();
    let (delivery, _rx) = lane(setup);
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                lease,
                frame,
                effect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
}
#[tokio::test]
async fn two_slots_bound_history_and_restore_partial_text_after_restart() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let mut projection = ReplayProjection::new("resp_1".into());
    projection.native_agent_id = Some("agt_native".into());
    let before = setup.server.memory.rows.lock().unwrap().len();
    for sequence in 0..20 {
        apply(
            &setup,
            &mut lease,
            &mut projection,
            &delta(sequence, "text"),
        )
        .await;
    }
    assert_eq!(setup.server.memory.rows.lock().unwrap().len(), before + 2);
    setup.journal().release(&mut lease).await.unwrap();
    let lease = accepted(&setup).await;
    let mut projection = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let body = setup.journal().body(&lease).await.unwrap();
    let effect = projection
        .project(&recovery_frame("resp_1", 20, true), &body, false, "failed")
        .unwrap();
    assert!(
        matches!(effect, ReplayEffect::Reply(ref message) if message.text.as_deref() == Some("text".repeat(20).as_str()))
    );
}
#[tokio::test]
async fn pending_projection_is_reused_and_changed_frame_is_refused() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    let original = delta(0, "once");
    projection
        .project(&original, &body, false, "failed")
        .unwrap();
    setup
        .journal()
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    let lease = accepted(&setup).await;
    let mut projection = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        projection.pending_effect(&original).unwrap(),
        Some(ReplayEffect::NoEffect)
    ));
    assert!(matches!(
        projection.pending_effect(&delta(0, "changed")),
        Err(ExecutionError::Corrupt)
    ));
    assert!(matches!(
        projection.project(&original, &body, false, "failed"),
        Err(ExecutionError::Invalid)
    ));
    let effect = projection
        .project(&recovery_frame("resp_1", 1, true), &body, false, "failed")
        .unwrap();
    assert!(
        matches!(effect, ReplayEffect::Reply(ref message) if message.text.as_deref() == Some("once"))
    );
}
#[tokio::test]
async fn failed_head_publication_preserves_previous_slot_and_no_cursor_advances() {
    for committed in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = accepted(&setup).await;
        let mut projection = ReplayProjection::new("resp_1".into());
        apply(&setup, &mut lease, &mut projection, &delta(0, "first")).await;
        let body = setup.journal().body(&lease).await.unwrap();
        projection
            .project(&delta(1, "second"), &body, false, "failed")
            .unwrap();
        setup
            .server
            .fault(setup.target().id.clone(), committed, false);
        let result = setup
            .journal()
            .save_projection(&mut lease, &projection)
            .await;
        if committed {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
        let restored = setup
            .journal()
            .load_projection(&lease)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.last.unwrap().sequence, u64::from(committed));
        assert!(matches!(
            lease.state.phase,
            Phase::Accepted {
                durable_cursor: Some(0),
                ..
            }
        ));
    }
}
#[tokio::test]
async fn encrypted_chunk_tampering_and_wrong_native_or_cursor_are_refused() {
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let mut projection = ReplayProjection::new("resp_1".into());
    apply(&setup, &mut lease, &mut projection, &delta(0, "private")).await;
    let id = setup
        .server
        .memory
        .rows
        .lock()
        .unwrap()
        .iter()
        .find_map(|(id, row)| {
            let opened =
                cmaeg_core::seal::open_interaction(&fixture::keys(), id, &row.blob).unwrap();
            let value: Value = serde_json::from_slice(&opened.payload).unwrap();
            (value["kind"] == "ordinary_route_projection_v1").then(|| id.clone())
        })
        .unwrap();
    setup
        .server
        .change(&id, |value| value["response_id"] = json!("foreign"));
    assert!(matches!(
        setup.journal().load_projection(&lease).await,
        Err(ExecutionError::Corrupt)
    ));
    projection.last.as_mut().unwrap().frame_hash = "0".repeat(64);
    assert!(matches!(
        setup
            .journal()
            .save_projection(&mut lease, &projection)
            .await,
        Err(ExecutionError::Corrupt)
    ));
    projection.native_agent_id = Some(String::new());
    assert!(matches!(
        setup
            .journal()
            .save_projection(&mut lease, &projection)
            .await,
        Err(ExecutionError::Corrupt)
    ));
}
#[tokio::test]
async fn projection_dedupes_committed_output_and_never_leaks_tools_reasoning_or_media() {
    let setup = Setup::new(supported()).await;
    let lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    for (sequence, item) in [
        json!({"type":"reasoning","id":"r","summary":[{"text":"secret"}]}),
        json!({"type":"function_call","id":"f","name":"secret_tool","arguments":"secret"}),
        json!({"type":"image_generation_call","id":"i","result":"secret"}),
    ]
    .into_iter()
    .enumerate()
    {
        let event = frame(
            json!({"type":"response.output_item.done","sequence_number":sequence,"item":item}),
        );
        assert!(matches!(
            projection.project(&event, &body, false, "failed").unwrap(),
            ReplayEffect::NoEffect
        ));
    }
    let event = frame(
        json!({"type":"response.output_text.done","sequence_number":3,"item_id":"m","text":"answer"}),
    );
    assert!(matches!(
        projection.project(&event, &body, false, "failed").unwrap(),
        ReplayEffect::Reply(_)
    ));
    let event = frame(
        json!({"type":"response.output_item.done","sequence_number":4,"item":{"type":"message","id":"m","content":[{"type":"output_text","text":"answer"}]}}),
    );
    assert!(matches!(
        projection.project(&event, &body, false, "failed").unwrap(),
        ReplayEffect::NoEffect
    ));
    assert!(matches!(
        projection
            .project(&recovery_frame("resp_1", 5, true), &body, false, "failed")
            .unwrap(),
        ReplayEffect::NoEffect
    ));
}

#[tokio::test]
async fn multichunk_snapshots_keep_complete_text_and_fence_an_expired_writer() {
    use std::sync::atomic::Ordering;
    let setup = Setup::new(supported()).await;
    let mut lease = accepted(&setup).await;
    let mut projection = ReplayProjection::new("resp_1".into());
    let chunk = "界".repeat(20_000);
    apply(&setup, &mut lease, &mut projection, &delta(0, &chunk)).await;
    let mut stale = Lease {
        state: lease.state.clone(),
    };
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut lease = accepted(&setup).await;
    let mut restored = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    apply(&setup, &mut lease, &mut restored, &delta(1, &chunk)).await;
    assert_eq!(
        setup
            .journal()
            .save_projection(&mut stale, &projection)
            .await,
        Err(ExecutionError::LostLease)
    );
    let mut latest = setup
        .journal()
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let body = setup.journal().body(&lease).await.unwrap();
    let effect = latest
        .project(&recovery_frame("resp_1", 2, true), &body, false, "failed")
        .unwrap();
    assert!(
        matches!(effect, ReplayEffect::Reply(ref message) if message.text.as_deref() == Some(chunk.repeat(2).as_str()))
    );
}

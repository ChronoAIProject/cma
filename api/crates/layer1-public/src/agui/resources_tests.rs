//! The resource lane's wire shape and merge behavior.

use super::*;

fn reading(value: Option<u64>) -> ResourceReading {
    ResourceReading {
        cpu_millis_used: value.map(|n| n as u32),
        cpu_millis_limit: value.map(|n| n as u32 + 1),
        memory_bytes_used: value.map(|n| n + 2),
        memory_bytes_limit: value.map(|n| n + 3),
        memory_bytes_peak: value.map(|n| n + 4),
        stall_permille: value.map(|n| n as u16 + 5),
        ceiling_hits: value.map(|n| n + 6),
        allocation_failures: value.map(|n| n + 7),
        kills: value.map(|n| n + 8),
        received_bytes: value.map(|n| n + 9),
        sent_bytes: value.map(|n| n + 10),
        received_bytes_per_second: value.map(|n| n + 11),
        sent_bytes_per_second: value.map(|n| n + 12),
        interval_ms: value.map(|n| n + 13),
    }
}

fn patch_value(frame: &AguiEvent) -> &serde_json::Value {
    &frame.body["delta"][0]["value"]
}

#[test]
fn a_reading_is_one_replace_on_one_path() {
    let frame = project_reading(&reading(Some(10)), "run_1", "sesn_1");
    let delta = frame.body["delta"].as_array().expect("a patch list");

    assert_eq!(frame.kind, "STATE_DELTA");
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0]["op"], "replace");
    assert_eq!(delta[0]["path"], "/resources");
}

#[test]
fn a_reading_projects_all_fourteen_fields_even_when_every_one_is_absent() {
    let frame = project_reading(&reading(None), "run_1", "sesn_1");
    let value = patch_value(&frame).as_object().expect("a reading object");
    let expected = [
        "allocationFailures",
        "ceilingHits",
        "cpuMillisLimit",
        "cpuMillisUsed",
        "intervalMs",
        "kills",
        "memoryBytesLimit",
        "memoryBytesPeak",
        "memoryBytesUsed",
        "receivedBytes",
        "receivedBytesPerSecond",
        "sentBytes",
        "sentBytesPerSecond",
        "stallPermille",
    ];

    assert_eq!(value.len(), expected.len(), "got {value:?}");
    for key in expected {
        assert!(value.contains_key(key), "missing {key}: {value:?}");
        assert!(value[key].is_null(), "{key} was not null: {value:?}");
    }
}

#[test]
fn an_absent_ceiling_and_first_link_rates_are_null_not_missing_or_zero() {
    let mut first = reading(Some(10));
    first.memory_bytes_limit = None;
    first.cpu_millis_used = None;
    first.received_bytes_per_second = None;
    first.sent_bytes_per_second = None;
    first.interval_ms = None;
    let frame = project_reading(&first, "run_1", "sesn_1");
    let value = patch_value(&frame);

    for key in [
        "memoryBytesLimit",
        "cpuMillisUsed",
        "receivedBytesPerSecond",
        "sentBytesPerSecond",
        "intervalMs",
    ] {
        assert!(value.get(key).is_some(), "missing {key}: {value}");
        assert!(value[key].is_null(), "{key} was not null: {value}");
    }
}

async fn on_the_wire(event: &AguiEvent) -> String {
    use axum::response::IntoResponse;
    let frame = super::super::project::sse_frame(event);
    let response = axum::response::sse::Sse::new(futures_util::stream::once(async move {
        Ok::<_, std::convert::Infallible>(frame)
    }))
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("the body is small");
    String::from_utf8(body.to_vec()).expect("frames are text")
}

#[tokio::test]
async fn a_reading_carries_no_cursor() {
    let frame = project_reading(&reading(Some(10)), "run_1", "sesn_1");
    assert!(frame.seq.is_none());
    assert!(frame.timestamp.is_none());

    let wire = on_the_wire(&frame).await;
    assert!(!wire.contains("id:"), "{wire:?}");
    assert!(wire.contains("\"cma:live\":true"), "{wire:?}");
}

#[tokio::test]
async fn both_live_lanes_are_selected_without_one_starving_the_other() {
    let (live_tx, live_rx) = tokio::sync::broadcast::channel(4);
    let (resource_tx, resource_rx) = tokio::sync::broadcast::channel(4);
    let mut live = Some(live_rx);
    let mut resources = Some(resource_rx);
    live_tx
        .send(LiveFrame::MessageDelta("part".into()))
        .expect("a live receiver");
    resource_tx
        .send(reading(Some(20)))
        .expect("a resource receiver");

    let first = next_frame(&mut live, &mut resources).await;
    let second = next_frame(&mut live, &mut resources).await;
    let woke_live = matches!(&first, Woke::Live(_)) || matches!(&second, Woke::Live(_));
    let woke_reading = matches!(&first, Woke::Reading(_)) || matches!(&second, Woke::Reading(_));

    assert!(woke_live, "the conversation lane was starved");
    assert!(woke_reading, "the resource lane was starved");
}

#[tokio::test]
async fn lagging_a_lane_skips_to_its_next_available_live_value() {
    let (live_tx, live_rx) = tokio::sync::broadcast::channel(1);
    let mut live = Some(live_rx);
    let mut resources = None;
    live_tx
        .send(LiveFrame::MessageDelta("old".into()))
        .expect("a receiver");
    live_tx
        .send(LiveFrame::MessageDelta("new".into()))
        .expect("a receiver");

    let woke = next_frame(&mut live, &mut resources).await;
    assert!(matches!(woke, Woke::Live(LiveFrame::MessageDelta(ref text)) if text == "new"));
}

#[tokio::test]
async fn lagging_the_resource_lane_skips_to_its_next_reading() {
    let (resource_tx, resource_rx) = tokio::sync::broadcast::channel(1);
    let mut live = None;
    let mut resources = Some(resource_rx);
    resource_tx.send(reading(Some(10))).expect("a receiver");
    resource_tx.send(reading(Some(20))).expect("a receiver");

    let woke = next_frame(&mut live, &mut resources).await;
    assert!(matches!(
        woke,
        Woke::Reading(ResourceReading {
            memory_bytes_used: Some(22),
            ..
        })
    ));
}

#[tokio::test]
async fn a_closed_resource_lane_leaves_conversation_liveness_working() {
    let (live_tx, live_rx) = tokio::sync::broadcast::channel(1);
    let (resource_tx, resource_rx) = tokio::sync::broadcast::channel(1);
    let mut live = Some(live_rx);
    let mut resources = Some(resource_rx);
    drop(resource_tx);
    let sender = tokio::spawn(async move {
        tokio::task::yield_now().await;
        live_tx
            .send(LiveFrame::MessageDelta("still here".into()))
            .expect("the live lane remains open");
    });

    let woke = next_frame(&mut live, &mut resources).await;
    sender.await.expect("sender finishes");
    assert!(matches!(woke, Woke::Live(LiveFrame::MessageDelta(ref text)) if text == "still here"));
    assert!(resources.is_none(), "the closed lane must be retired");
}

use super::*;
use layer2_core::sandbox::store::AgentLogStore;
use std::sync::atomic::Ordering;
use tokio::time::Instant;
#[path = "stream_wake_probe.rs"]
mod probe;
use probe::Probe;

async fn prepared(enabled: bool) -> (Following, Fixture, Arc<Probe>) {
    let (mut open, f) = following().await;
    // The first accepted output commits prompt evidence. Measure the caught-up
    // steady state only after that durable transition and its live fragment.
    publish(&open, "fixture readiness").await;
    open.fill().await.unwrap();
    open.pending.clear();
    while open.watching.as_mut().unwrap().try_recv().is_ok() {}
    open.state = open.state.with_event_wake(enabled, Duration::from_secs(1));
    open.heads = enabled.then(|| {
        f.state
            .updates
            .as_ref()
            .unwrap()
            .subscribe_head(&open.header.id)
            .unwrap()
    });
    let probe = Probe::new(f.store.clone());
    open.state.reads = probe.reads();
    open.fallback_due = Instant::now() + Duration::from_secs(1);
    (open, f, probe)
}
async fn publish(open: &Following, text: &str) {
    open.state
        .updates
        .as_ref()
        .unwrap()
        .record_update(
            &open.header.id,
            open.header.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            AgentRuntimeUpdate::Said(
                serde_json::from_value(serde_json::json!({
                    "sessionUpdate":"agent_message_chunk", "messageId":"wake-live",
                    "content":{"type":"text","text":text}
                }))
                .unwrap(),
            ),
            "2026-09-16T00:00:00Z".into(),
        )
        .await
        .unwrap();
}
async fn next_wire(
    body: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
) -> String {
    String::from_utf8(body.next().await.unwrap().unwrap().to_vec()).unwrap()
}

#[tokio::test(start_paused = true)]
async fn immediate_live_delivery_uses_one_snapshot_and_zero_log_reads() {
    let (open, _, probe) = prepared(true).await;
    publish(&open, "immediate").await;
    let start = Instant::now();
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("immediate"));
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 1);
    assert_eq!(probe.logs.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn delayed_snapshot_delivery_benchmark_baseline_and_wake() {
    for enabled in [false, true] {
        let mut samples = Vec::new();
        for _ in 0..20 {
            let (mut open, _, probe) = prepared(enabled).await;
            open.live_timestamps = true;
            probe.delay_ms.store(100, Ordering::Relaxed);
            publish(&open, "benchmark").await;
            let start = Instant::now();
            let mut body = response(open).into_body().into_data_stream();
            let wire = next_wire(&mut body).await;
            assert!(wire.contains("benchmark") && wire.contains("observed_at"));
            samples.push(start.elapsed().as_millis());
            assert_eq!(
                probe.snapshots.load(Ordering::Relaxed),
                if enabled { 1 } else { 4 }
            );
            assert_eq!(probe.logs.load(Ordering::Relaxed) == 0, enabled);
        }
        samples.sort_unstable();
        let p95 = samples[18];
        eprintln!(
            "event-wake fixture enabled={enabled} samples=20 snapshot_delay_ms=100 p95_ms={p95}"
        );
        if enabled {
            assert!(p95 <= 150);
        } else {
            assert!(p95 >= 500);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn idle_fallback_uses_one_snapshot_per_interval_without_log_work() {
    for multiplier in [1, 10] {
        let (mut open, f, probe) = prepared(true).await;
        let extra_history = open.header.head * (multiplier - 1);
        for _ in 0..extra_history {
            append(
                &f.store,
                AgentLogKind::OwnerMessage {
                    content: "older".into(),
                },
            )
            .await;
        }
        if extra_history > 0 {
            open.fill().await.unwrap();
            open.pending.clear();
        }
        probe.logs.store(0, Ordering::Relaxed);
        probe.snapshots.store(0, Ordering::Relaxed);
        let start = Instant::now();
        for _ in 0..10 {
            open.wait_for_change().await.unwrap();
            assert!(open.pending.is_empty());
        }
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert_eq!(probe.snapshots.load(Ordering::Relaxed), 10);
        assert_eq!(probe.logs.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn missing_hint_discovers_durable_append_at_fixed_fallback() {
    let (open, f, probe) = prepared(true).await;
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "remote append".into(),
        },
    )
    .await;
    let start = Instant::now();
    let mut body = response(open).into_body().into_data_stream();
    let wire = next_wire(&mut body).await;
    assert!(wire.contains("cma.agent.event") && wire.contains("remote append"));
    assert_eq!(start.elapsed(), Duration::from_secs(1));
    assert!(probe.logs.load(Ordering::Relaxed) > 0);
}

#[tokio::test(start_paused = true)]
async fn local_commit_wakes_before_fallback_and_precedes_stale_live() {
    let (open, _, _) = prepared(true).await;
    publish(&open, "committed answer").await;
    open.state
        .updates
        .as_ref()
        .unwrap()
        .record_update(
            &open.header.id,
            1,
            &mut AgentMessageAssembly::default(),
            AgentRuntimeUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            "2026-09-16T00:00:00Z".into(),
        )
        .await
        .unwrap();
    let start = Instant::now();
    let mut body = response(open).into_body().into_data_stream();
    let wire = next_wire(&mut body).await;
    assert!(wire.contains("cma.agent.event"));
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn live_racing_durable_head_delivers_durable_first() {
    let (mut open, f, _) = prepared(true).await;
    publish(&open, "live after steering").await;
    open.queue_live(false);
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "steering first".into(),
        },
    )
    .await;
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("steering first"));
    assert!(next_wire(&mut body).await.contains("live after steering"));
}

#[tokio::test(start_paused = true)]
async fn future_anchor_waits_without_spinning_then_follows_catchup() {
    let (mut open, f, probe) = prepared(true).await;
    let mut future = fragment(&open);
    future.after_seq += 1;
    open.held = Some(future.into());
    let start = Instant::now();
    open.wait_for_change().await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(1));
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 1);
    assert!(open.pending.is_empty() && open.held.is_some());
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "anchor".into(),
        },
    )
    .await;
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("anchor"));
    assert!(next_wire(&mut body).await.contains("Live text"));
}

#[tokio::test(start_paused = true)]
async fn deletion_and_snapshot_failure_close_before_releasing_live() {
    for failure in [false, true] {
        for live in [false, true] {
            let (open, _, probe) = prepared(true).await;
            if live {
                publish(&open, "must not escape").await;
            }
            if failure {
                probe.failed.store(true, Ordering::Relaxed);
            } else {
                probe.deleted.store(true, Ordering::Relaxed);
            }
            let start = Instant::now();
            let mut body = response(open).into_body().into_data_stream();
            assert!(body.next().await.is_none());
            assert!(start.elapsed() <= Duration::from_secs(1));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn held_consumer_does_no_work_and_keeps_all_durable_frames() {
    let (open, f, probe) = prepared(true).await;
    let mut body = response(open).into_body().into_data_stream();
    for content in ["durable one", "durable two", "durable three"] {
        append(
            &f.store,
            AgentLogKind::OwnerMessage {
                content: content.into(),
            },
        )
        .await;
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(probe.snapshots.load(Ordering::Relaxed), 0);
    assert_eq!(probe.logs.load(Ordering::Relaxed), 0);
    for content in ["durable one", "durable two", "durable three"] {
        assert!(next_wire(&mut body).await.contains(content));
    }
}

#[tokio::test(start_paused = true)]
async fn changed_head_keeps_tail_integrity_failure_visible() {
    let (open, f, _) = prepared(true).await;
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "committed".into(),
        },
    )
    .await;
    f.store
        .append_agent_log(
            &open.header.id,
            "2026-09-16T00:00:00Z".into(),
            AgentLogKind::AgentMessage {
                content: "uncommitted".into(),
            },
        )
        .await
        .unwrap();
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("cma.agent.error"));
    assert!(body.next().await.is_none());
}

#[path = "stream_wake_race_tests.rs"]
mod races;

#[tokio::test(start_paused = true)]
async fn buffered_gap_resumes_immediately_after_durable_catchup() {
    let (mut open, f, _) = prepared(true).await;
    open.discarded_fragments = 3;
    open.watching = None;
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "before gap".into(),
        },
    )
    .await;
    open.fill().await.unwrap();
    let start = Instant::now();
    let mut body = response(open).into_body().into_data_stream();
    assert!(next_wire(&mut body).await.contains("before gap"));
    let wire = next_wire(&mut body).await;
    assert!(wire.contains("cma.agent.live_gap") && wire.contains("\"frames\":3"));
    assert_eq!(start.elapsed(), Duration::ZERO);
}

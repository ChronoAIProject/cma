use super::*;
use crate::{
    delivery::sync::{SyncRegistry, Waiter},
    turns::routed::{delivery_tests::accepted, test_support::*, Phase},
};
use axum::response::{IntoResponse, Sse};
use std::sync::atomic::Ordering;

async fn setup() -> (Setup, StreamMonitor, Waiter) {
    let waiters = Arc::new(SyncRegistry::default().with_instance("a".repeat(64)));
    let (id, waiter) = waiters.register().unwrap();
    let mut request = fixture::request(
        |value| {
            fixture::targets(value);
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        fixture::custom(None),
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync { waiter: id.clone() };
    });
    let setup = Setup::with_request(supported(), request).await;
    let monitor = StreamMonitor::new(
        setup.journal().clone(),
        CustomDelivery::new("https://gateway.example".into(), waiters),
    );
    (setup, monitor, waiter)
}

#[tokio::test]
async fn queued_and_long_running_work_have_no_short_total_stream_deadline() {
    let (setup, monitor, waiter) = setup().await;
    assert!(!monitor.observe(&setup.plan, setup.target()).await.unwrap());
    let _lease = accepted(&setup).await;
    assert!(!monitor.observe(&setup.plan, setup.target()).await.unwrap());
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 600, Ordering::SeqCst);
    assert!(!monitor.observe(&setup.plan, setup.target()).await.unwrap());
    assert_eq!(monitor.custom.waiters.len(), 1);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    drop(waiter);
}

#[tokio::test]
async fn failed_status_read_ends_with_uncertainty_without_rewriting_or_retrying_work() {
    let (setup, monitor, waiter) = setup().await;
    let _lease = accepted(&setup).await;
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    setup
        .server
        .memory
        .unavailable_reads
        .store(100, Ordering::SeqCst);
    monitor.start(&setup.plan);
    let bytes = tokio::time::timeout(
        Duration::from_secs(3),
        axum::body::to_bytes(Sse::new(waiter.stream()).into_response().into_body(), 4096),
    )
    .await
    .unwrap()
    .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.matches("event: cmaeg.failed").count(), 1);
    assert!(text.contains("sync_delivery_observation_unavailable"));
    assert!(!text.contains("event: cmaeg.completed"));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    setup
        .server
        .memory
        .unavailable_reads
        .store(0, Ordering::SeqCst);
    let state = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(matches!(state.phase, Phase::Accepted { .. }));
    assert!(monitor.custom.waiters.is_empty());
}

#[tokio::test]
async fn monitoring_is_limited_to_one_original_live_ordinary_waiter() {
    let (setup, monitor, waiter) = setup().await;
    let ReplyAddress::Sync { waiter: id } = &setup.target().launch.event.reply else {
        panic!("sync fixture");
    };
    assert!(monitor.custom.waiters.monitor(id));
    assert!(!monitor.custom.waiters.monitor(id));
    drop(waiter);
    assert!(!monitor.custom.waiters.monitor(id));
    let (media_id, _media) = monitor
        .custom
        .waiters
        .register_with_media_responses(true)
        .unwrap();
    assert!(!monitor.custom.waiters.monitor(&media_id));
}

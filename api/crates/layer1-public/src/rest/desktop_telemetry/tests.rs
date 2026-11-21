use super::*;
use std::{
    io::{self, Write},
    sync::Arc,
};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn emits_exactly_one_content_free_summary_and_balances_the_active_gauge() {
    let output = Output::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let counters = Counters::default();
    let mut relay = RelayObservation::new(&counters);
    assert_eq!(counters.active.load(Relaxed), 1);
    relay.send_wait(Direction::Uplink, Duration::from_millis(8));
    relay.forwarded(Direction::Uplink, 5);
    relay.send_wait(Direction::Downlink, Duration::from_millis(22));
    relay.forwarded(Direction::Downlink, 7);
    relay.send_wait(Direction::Downlink, Duration::from_secs(10));
    relay.authorization_check(true, Duration::from_millis(1), None);
    relay.authorization_check(false, Duration::from_millis(2), None);
    tokio::time::advance(Duration::from_secs(12)).await;
    relay.finish(CloseReason::ClientSendTimeout);

    let cancelled = RelayObservation::new(&counters);
    assert_eq!(counters.active.load(Relaxed), 1);
    drop(cancelled);
    assert_eq!(counters.active.load(Relaxed), 0);
    assert_eq!(
        counters
            .closed
            .iter()
            .map(|count| count.load(Relaxed))
            .sum::<u64>(),
        1
    );
    assert_eq!(
        counters.closed[CloseReason::ClientSendTimeout as usize].load(Relaxed),
        1
    );
    assert_eq!(
        counters.directions[Direction::Uplink as usize]
            .frames
            .load(Relaxed),
        1
    );
    assert_eq!(
        counters.directions[Direction::Uplink as usize]
            .bytes
            .load(Relaxed),
        5
    );
    assert_eq!(
        counters.directions[Direction::Downlink as usize]
            .frames
            .load(Relaxed),
        1
    );
    assert_eq!(
        counters.directions[Direction::Downlink as usize]
            .bytes
            .load(Relaxed),
        7
    );

    let bytes = output.0.lock().unwrap();
    let lines: Vec<_> = std::str::from_utf8(&bytes).unwrap().lines().collect();
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        record["fields"],
        serde_json::json!({
            "message": "desktop relay ended",
            "reason": "client_send_timeout",
            "elapsed_ms": 12000.0,
            "uplink_frames": 1,
            "uplink_bytes": 5,
            "uplink_max_send_wait_ms": 8.0,
            "downlink_frames": 1,
            "downlink_bytes": 7,
            "downlink_max_send_wait_ms": 10000.0,
            "authorization_checks": 2,
            "max_authorization_wait_ms": 2.0,
        })
    );
}

#[test]
fn publishes_bounded_cumulative_send_wait_buckets_for_each_direction() {
    let counters = Counters::default();
    let mut relay = RelayObservation::new(&counters);
    relay.send_wait(Direction::Uplink, Duration::from_millis(1));
    for wait in [
        Duration::from_millis(20),
        Duration::from_millis(20) + Duration::from_nanos(1),
        Duration::from_millis(100),
        Duration::from_secs(10),
        Duration::from_secs(11),
    ] {
        relay.send_wait(Direction::Downlink, wait);
    }

    let mut scrape = String::new();
    counters.append_send_histograms(&mut scrape);
    assert!(
        scrape.contains("cma_desktop_relay_send_wait_seconds_count{direction=\"downlink\"} 0\n")
    );

    counters.emit();
    scrape.clear();
    counters.append_send_histograms(&mut scrape);
    assert!(scrape.contains("# TYPE cma_desktop_relay_send_wait_seconds histogram\n"));
    for line in [
        "_bucket{direction=\"downlink\",le=\"0.02\"} 1",
        "_bucket{direction=\"downlink\",le=\"0.05\"} 2",
        "_bucket{direction=\"downlink\",le=\"0.1\"} 3",
        "_bucket{direction=\"downlink\",le=\"10\"} 4",
        "_bucket{direction=\"downlink\",le=\"+Inf\"} 5",
        "_sum{direction=\"downlink\"} 21.140000001",
        "_count{direction=\"downlink\"} 5",
        "_count{direction=\"uplink\"} 1",
    ] {
        assert!(
            scrape.contains(&format!("{SEND_WAIT_SECONDS}{line}\n")),
            "{line}"
        );
    }
    assert_eq!(scrape.lines().count(), 1 + 2 * (BUCKET_COUNT + 2));
}

#[test]
fn metric_labels_are_the_issue_contract() {
    assert_eq!(
        CloseReason::ALL.map(CloseReason::label),
        [
            "client_closed",
            "client_error",
            "runtime_closed",
            "runtime_error",
            "client_send_timeout",
            "runtime_send_timeout",
            "authorization_refused",
            "lifetime"
        ]
    );
    assert_eq!(Direction::ALL.map(Direction::label), ["uplink", "downlink"]);
}

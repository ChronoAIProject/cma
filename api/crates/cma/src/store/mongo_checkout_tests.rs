use super::*;

fn checkout(id: u32) -> CmapEvent {
    CmapEvent::ConnectionCheckedOut(checkout_event(id))
}

fn checkout_event(id: u32) -> mongodb::event::cmap::ConnectionCheckedOutEvent {
    let mut event: mongodb::event::cmap::ConnectionCheckedOutEvent =
        serde_json::from_value(serde_json::json!({ "connectionId": id })).unwrap();
    event.duration = Duration::from_millis(125);
    event
}

fn checkout_at(address: &str, id: u32) -> CmapEvent {
    let mut event = checkout_event(id);
    event.address = mongodb::options::ServerAddress::parse(address).unwrap();
    CmapEvent::ConnectionCheckedOut(event)
}

#[test]
fn checkout_classification_uses_the_server_limit_across_summary_windows() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let first = "first.example:27017";
    let second = "second.example:27017";
    metrics::with_local_recorder(&recorder, || {
        let observer = Observer(
            Arc::new(Mutex::new(Window::new(Instant::now()))),
            "application",
            2,
        );
        record_event(&observer, checkout_at(first, 1));
        record_event(&observer, checkout_at(second, 1));
        observer.0.lock().unwrap().started = Instant::now() - WINDOW;
        observer.record(2, Duration::ZERO, false);
        record_event(&observer, checkout_at(first, 2));
        let mut returned: mongodb::event::cmap::ConnectionCheckedInEvent =
            serde_json::from_value(serde_json::json!({ "connectionId": 1 })).unwrap();
        returned.address = mongodb::options::ServerAddress::parse(second).unwrap();
        record_event(&observer, CmapEvent::ConnectionCheckedIn(returned));
        record_event(&observer, checkout_at(second, 2));
        let mut closed: mongodb::event::cmap::ConnectionClosedEvent =
            serde_json::from_value(serde_json::json!({ "connectionId": 2, "reason": "error" }))
                .unwrap();
        closed.address = mongodb::options::ServerAddress::parse(first).unwrap();
        record_event(&observer, CmapEvent::ConnectionClosed(closed));
        record_event(&observer, checkout_at(first, 3));
    });
    let text = handle.render();
    count(&text, "below_limit", 3);
    count(&text, "at_limit", 2);
    assert!(text.contains("cma_mongo_connections_tracked{pool=\"application\"} 3"));
    assert!(!text.contains(first) && !text.contains(second), "{text}");
}

fn count(text: &str, state: &str, expected: u64) {
    assert!(text.contains(&format!(
        "cma_mongo_checkout_wait_seconds_count{{pool=\"application\",pool_state=\"{state}\"}} {expected}"
    )), "{text}");
}

#[test]
fn successful_checkouts_use_occupancy_before_insertion_and_configured_limit() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let callback = observer(
            &ClientOptions::builder().max_pool_size(2).build(),
            "application",
        );
        callback(checkout(1));
        callback(checkout(2));
        let closed = serde_json::from_value(serde_json::json!({
            "connectionId": 1, "reason": "error"
        }))
        .unwrap();
        callback(CmapEvent::ConnectionClosed(closed));
        let returned = serde_json::from_value(serde_json::json!({ "connectionId": 2 })).unwrap();
        callback(CmapEvent::ConnectionCheckedIn(returned));
        let failed = serde_json::from_value(serde_json::json!({ "reason": "timeout" })).unwrap();
        callback(CmapEvent::ConnectionCheckoutFailed(failed));
        callback(checkout(3));
    });
    let text = handle.render();
    count(&text, "below_limit", 2);
    count(&text, "at_limit", 1);
    assert!(!text.contains("pool_state=\"unknown\""));
    assert!(text.contains("cma_mongo_connections_tracked{pool=\"application\"} 1"));
    assert!(text.contains("cma_mongo_pool_operation_seconds_count{pool=\"application\",operation=\"checkout\",outcome=\"failed\"} 1"));
    assert!(text.contains(
        "cma_mongo_checkout_wait_seconds_sum{pool=\"application\",pool_state=\"below_limit\"} 0.25"
    ));
}

#[test]
fn overflow_is_unknown_even_after_window_rotation_and_unmatched_returns() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let observer = Observer(
            Arc::new(Mutex::new(Window::new(Instant::now()))),
            "application",
            30,
        );
        for id in 1..=MAX_TRACKED_CONNECTIONS as u32 + 1 {
            record_event(&observer, checkout(id));
        }
        // Returning an untracked connection cannot prove that all other lost
        // pairing information has been recovered.
        let returned = serde_json::from_value(serde_json::json!({
            "connectionId": MAX_TRACKED_CONNECTIONS as u32 + 1
        }))
        .unwrap();
        record_event(&observer, CmapEvent::ConnectionCheckedIn(returned));
        {
            let mut window = observer.0.lock().unwrap();
            window.started = Instant::now() - WINDOW;
            window.checked_out.clear();
            window.server_in_use.clear();
        }
        observer.record(1, Duration::ZERO, false);
        assert_eq!(observer.0.lock().unwrap().untracked, 0);
        record_event(&observer, checkout(999_999));
    });
    let text = handle.render();
    count(&text, "unknown", 2);
    count(&text, "below_limit", 29);
    for private in [
        "999999",
        "localhost",
        "agent_id",
        "sandbox_id",
        "request_id",
        "user_id",
    ] {
        assert!(!text.contains(private), "{private}");
    }
}

#[test]
fn absent_limit_uses_the_existing_ten_connection_default() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let callback = observer(&ClientOptions::default(), "application");
        for id in 1..=10 {
            callback(checkout(id));
        }
    });
    let text = handle.render();
    count(&text, "below_limit", 9);
    count(&text, "at_limit", 1);
}

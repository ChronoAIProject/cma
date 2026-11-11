use super::*;

#[test]
fn scrape_observes_checkout_failures_and_occupancy_without_connection_identifiers() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
        observer.checked_out(424242, Duration::from_millis(12));
        assert!(handle.render().contains("cma_mongo_connections_tracked 1"));
        observer.returned(424242, false);
        observer.record(0, Duration::from_millis(30), true);
    });
    let text = handle.render();
    assert!(text.contains("cma_mongo_connections_tracked 0"));
    assert!(
        text.contains("cma_mongo_pool_events_total{operation=\"checkout\",outcome=\"failed\"} 1")
    );
    assert!(text.contains(
        "cma_mongo_pool_operation_seconds_count{operation=\"checkout\",outcome=\"ok\"} 1"
    ));
    assert!(!text.contains("424242"));
}
use std::io::Write;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[test]
fn distribution_keeps_failures_and_non_overlapping_latency_buckets() {
    let mut window = Window::new(Instant::now());
    for elapsed in [
        0, 1_000, 1_001, 5_000, 5_001, 25_000, 25_001, 100_000, 100_001, 250_000, 250_001,
        1_000_000, 1_000_001,
    ] {
        window.record(0, Duration::from_micros(elapsed), elapsed == 1_000_001);
    }
    assert_eq!(window.samples[0].count, 13);
    assert_eq!(window.samples[0].failures, 1);
    assert_eq!(window.samples[0].buckets, [2, 2, 2, 2, 2, 2, 1]);
    assert_eq!(window.samples[0].max_us, 1_000_001);
    assert_eq!(window.samples[1], Sample::default());
}

#[test]
fn summary_is_bounded_and_excludes_parent_span_payloads() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
    let untrusted = "private query text must never enter logs";
    let span = tracing::info_span!("caller", private_context = untrusted);
    let _entered = span.enter();
    for _ in 0..10_000 {
        observer.record(0, Duration::from_millis(50), false);
    }
    assert!(output.0.lock().unwrap().is_empty());
    observer.0.lock().unwrap().started = Instant::now() - WINDOW;
    observer.record(0, Duration::from_millis(2), true);
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 1);
    assert!(logs.contains("checkout") && logs.contains("count=10001"));
    assert!(logs.contains("failures=1"));
    assert!(!logs.contains(untrusted));
    assert!(observer
        .0
        .lock()
        .unwrap()
        .samples
        .iter()
        .all(|sample| *sample == Sample::default()));
    observer.record(0, Duration::ZERO, false);
    assert_eq!(
        String::from_utf8(output.0.lock().unwrap().clone()).unwrap(),
        logs
    );
}

#[test]
fn diagnostic_counter_overflow_cannot_panic_or_change_database_behavior() {
    let mut window = Window::new(Instant::now());
    window.samples[0] = Sample {
        count: u64::MAX,
        failures: u64::MAX,
        total_us: u64::MAX,
        max_us: 0,
        buckets: [u64::MAX; 7],
    };
    window.record(0, Duration::MAX, true);
    assert_eq!(window.samples[0].count, u64::MAX);
    assert_eq!(window.samples[0].failures, u64::MAX);
    assert_eq!(window.samples[0].max_us, u64::MAX);
    assert_eq!(window.samples[0].total_us, u64::MAX);
}

#[test]
fn connection_holds_survive_windows_and_closed_connections_release_tracking() {
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
    observer.checked_out(1, Duration::ZERO);
    observer.checked_out(2, Duration::ZERO);
    {
        let mut window = observer.0.lock().unwrap();
        window.started = Instant::now() - WINDOW;
        window
            .checked_out
            .insert(1, Instant::now() - Duration::from_secs(12));
        assert_eq!(window.in_use_peak, 2);
    }
    observer.record(0, Duration::ZERO, false);
    assert_eq!(observer.0.lock().unwrap().checked_out.len(), 2);
    observer.returned(1, false);
    observer.returned(2, true);
    observer.returned(2, true);
    let window = observer.0.lock().unwrap();
    assert!(window.checked_out.is_empty());
    assert_eq!(window.samples[3].count, 2);
    assert_eq!(window.samples[3].failures, 1);
    assert!(window.samples[3].max_us >= 12_000_000);
}

#[test]
fn connection_tracking_is_bounded_and_separates_server_local_ids() {
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
    for key in 0..MAX_TRACKED_CONNECTIONS as u64 + 2 {
        observer.checked_out(key, Duration::ZERO);
    }
    let window = observer.0.lock().unwrap();
    assert_eq!(window.checked_out.len(), MAX_TRACKED_CONNECTIONS);
    assert_eq!(window.untracked, 2);
    drop(window);
    let left = mongodb::options::ServerAddress::parse("first.example:27017").unwrap();
    let right = mongodb::options::ServerAddress::parse("second.example:27017").unwrap();
    assert_ne!(connection_key(&left, 1), connection_key(&right, 1));
    assert_ne!(connection_key(&left, 1), connection_key(&left, 2));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn real_driver_observations_preserve_operations_and_exclude_payloads() {
    use mongodb::{
        bson::{doc, Document},
        Client,
    };
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
    let mut options = ClientOptions::parse(uri).await.unwrap();
    attach(&mut options, observer.clone());
    // Installing a command callback would eagerly decode full BSON payloads
    // inside the driver even if our callback ignores them.
    assert!(options.command_event_handler.is_none());
    assert!(options.cmap_event_handler.is_some());
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_latency_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let collection = database.collection::<Document>("private_records");
    let sentinel = "private diagnostic payload must not be logged";
    collection
        .insert_one(doc! { "_id": "one", "value": sentinel })
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    collection
        .update_one(doc! { "_id": "one" }, doc! { "$set": { "checked": true } })
        .session(&mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
    let value = collection
        .find_one(doc! { "_id": "one" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.get_str("value").unwrap(), sentinel);
    assert!(value.get_bool("checked").unwrap());
    assert!(collection.insert_one(doc! { "_id": "one" }).await.is_err());
    assert!(database.run_command(doc! { sentinel: 1 }).await.is_err());
    database.drop().await.unwrap();
    // Driver callbacks are synchronous. Reading these counters needs no extra
    // production command, query, collection identifier, or credential access.
    let samples = observer.0.lock().unwrap().samples;
    assert!(samples[0].count > 0 && samples[1].count > 0);
    // A rejected database command is not a connection checkout failure.
    assert_eq!(samples[0].failures, 0);
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    observer.0.lock().unwrap().started = Instant::now() - WINDOW;
    observer.record(0, Duration::ZERO, false);
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("MongoDB client latency summary"));
    for private in [sentinel, &name, "private_records", "127.0.0.1"] {
        assert!(!logs.contains(private));
    }
}

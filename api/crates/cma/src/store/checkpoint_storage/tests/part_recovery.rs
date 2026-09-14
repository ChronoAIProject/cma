use super::*;
use tracing::instrument::WithSubscriber;

const KEY: &str = "private-checkpoint-part";
const BYTES: &[u8] = b"immutable checkpoint bytes";

#[derive(Clone)]
struct TraceWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TraceWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn faults(fixture: &Fixture, method: &str, key: &str, faults: Vec<Fault>) {
    fixture
        .faults
        .lock()
        .unwrap()
        .insert((method.into(), key.into()), faults.into());
}

fn methods(fixture: &Fixture, key: &str) -> Vec<String> {
    fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, target)| target == key)
        .map(|(method, _)| method.clone())
        .collect()
}

async fn wait_for_calls(fixture: &Fixture, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let changed = fixture.requests_changed.notified();
            if fixture.calls.lock().unwrap().len() >= count {
                return;
            }
            changed.await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ambiguous_committed_posts_are_reconciled_without_duplicate_upload() {
    for disconnect in [false, true] {
        let (store, fixture, server) = fixture().await;
        let mut fault = Fault::status(StatusCode::SERVICE_UNAVAILABLE);
        fault.commit = true;
        fault.disconnect = disconnect;
        faults(&fixture, "POST", KEY, vec![fault]);
        assert_eq!(
            part_transfer::upload(&store, KEY, BYTES.to_vec())
                .await
                .unwrap(),
            format!("{:x}", Sha256::digest(BYTES))
        );
        assert_eq!(methods(&fixture, KEY), ["POST", "GET"]);
        assert_eq!(fixture.objects.lock().unwrap()[KEY], BYTES);
        server.abort();
    }
}

#[tokio::test]
async fn failure_diagnostics_include_only_safe_stage_status_attempt_and_key_digest() {
    let (store, fixture, server) = fixture().await;
    let mut fault = Fault::status(StatusCode::SERVICE_UNAVAILABLE);
    fault.commit = true;
    fault.body = Some(b"private-response-body".to_vec());
    faults(&fixture, "POST", KEY, vec![fault]);
    faults(
        &fixture,
        "GET",
        KEY,
        vec![Fault::status(StatusCode::FORBIDDEN)],
    );
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = TraceWriter(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let error = part_transfer::upload(&store, KEY, BYTES.to_vec())
        .with_subscriber(subscriber)
        .await
        .unwrap_err();
    let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    for expected in [
        "stage=\"upload\"",
        "status=503",
        "attempt=1",
        "stage=\"readback\"",
        "status=403",
    ] {
        assert!(
            logs.contains(expected),
            "missing safe diagnostic {expected}"
        );
    }
    assert!(logs.contains(&format!("{:x}", Sha256::digest(KEY.as_bytes()))));
    for private in [
        KEY,
        "127.0.0.1",
        "key=",
        "fixture-material",
        "fixture-storage-bearer",
        "private-response-body",
        std::str::from_utf8(BYTES).unwrap(),
    ] {
        assert!(
            !logs.contains(private),
            "diagnostics exposed private transfer context"
        );
        assert!(!error.to_string().contains(private));
    }
    server.abort();
}

#[tokio::test]
async fn missing_uncertain_post_retries_the_same_key_and_bytes() {
    let (store, fixture, server) = fixture().await;
    faults(
        &fixture,
        "POST",
        KEY,
        vec![Fault::status(StatusCode::TOO_MANY_REQUESTS)],
    );
    part_transfer::upload(&store, KEY, BYTES.to_vec())
        .await
        .unwrap();
    assert_eq!(methods(&fixture, KEY), ["POST", "GET", "POST", "GET"]);
    assert_eq!(fixture.objects.lock().unwrap()[KEY], BYTES);
    assert_eq!(fixture.objects.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn successful_post_retries_only_readback_after_status_or_transport_failure() {
    for disconnect in [false, true] {
        let (store, fixture, server) = fixture().await;
        let mut fault = Fault::status(StatusCode::BAD_GATEWAY);
        fault.disconnect = disconnect;
        faults(&fixture, "GET", KEY, vec![fault]);
        part_transfer::upload(&store, KEY, BYTES.to_vec())
            .await
            .unwrap();
        assert_eq!(methods(&fixture, KEY), ["POST", "GET", "GET"]);
        server.abort();
    }
}

#[tokio::test]
async fn uncertain_post_with_transient_reconciliation_never_reposts_until_absence() {
    let (store, fixture, server) = fixture().await;
    let mut fault = Fault::status(StatusCode::GATEWAY_TIMEOUT);
    fault.commit = true;
    faults(&fixture, "POST", KEY, vec![fault]);
    faults(
        &fixture,
        "GET",
        KEY,
        vec![Fault::status(StatusCode::INTERNAL_SERVER_ERROR)],
    );
    part_transfer::upload(&store, KEY, BYTES.to_vec())
        .await
        .unwrap();
    assert_eq!(methods(&fixture, KEY), ["POST", "GET", "GET"]);
    server.abort();
}

#[tokio::test]
async fn retry_exhaustion_is_bounded_for_posts_and_confirmed_readbacks() {
    for method in ["POST", "GET"] {
        let (store, fixture, server) = fixture().await;
        faults(
            &fixture,
            method,
            KEY,
            (0..3)
                .map(|_| Fault::status(StatusCode::SERVICE_UNAVAILABLE))
                .collect(),
        );
        let error = part_transfer::upload(&store, KEY, BYTES.to_vec())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("retry budget exhausted"));
        let calls = methods(&fixture, KEY);
        assert_eq!(calls.iter().filter(|call| *call == method).count(), 3);
        if method == "GET" {
            assert_eq!(calls, ["POST", "GET", "GET", "GET"]);
        } else {
            assert_eq!(calls, ["POST", "GET", "POST", "GET", "POST", "GET"]);
        }
        server.abort();
    }
}

#[tokio::test]
async fn permanent_statuses_and_confirmed_missing_objects_fail_closed_without_retry() {
    for method in ["POST", "GET"] {
        for status in [401, 403, 400, 404, 409, 413, 501, 505, 307] {
            let (store, fixture, server) = fixture().await;
            faults(
                &fixture,
                method,
                KEY,
                vec![Fault::status(StatusCode::from_u16(status).unwrap())],
            );
            let error = part_transfer::upload(&store, KEY, BYTES.to_vec())
                .await
                .unwrap_err();
            let rendered = error.to_string();
            assert!(rendered.contains(&format!("HTTP {status}")));
            for private in [
                KEY,
                "127.0.0.1",
                "fixture-material",
                "fixture-storage-bearer",
            ] {
                assert!(!rendered.contains(private));
            }
            assert_eq!(
                methods(&fixture, KEY),
                if method == "POST" {
                    vec!["POST"]
                } else {
                    vec!["POST", "GET"]
                }
            );
            assert_eq!(fixture.tokens.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }
}

#[tokio::test]
async fn integrity_and_size_errors_never_retry_or_acknowledge() {
    for (chunked, body) in [false, true].into_iter().flat_map(|chunked| {
        [
            vec![0; BYTES.len()],
            vec![0; BYTES.len() - 1],
            vec![0; BYTES.len() + 1],
        ]
        .into_iter()
        .map(move |body| (chunked, body))
    }) {
        let (store, fixture, server) = fixture().await;
        let mut fault = Fault::status(StatusCode::OK);
        fault.body = Some(body);
        fault.chunked = chunked;
        faults(&fixture, "GET", KEY, vec![fault]);
        assert!(part_transfer::upload(&store, KEY, BYTES.to_vec())
            .await
            .is_err());
        assert_eq!(methods(&fixture, KEY), ["POST", "GET"]);
        server.abort();
    }
}

#[tokio::test]
async fn unusable_token_and_invalid_part_size_fail_before_object_request() {
    let (store, fixture, server) = fixture().await;
    for bytes in [Vec::new(), vec![0; multipart::PART_BYTES + 1]] {
        assert!(part_transfer::upload(&store, KEY, bytes).await.is_err());
    }
    fixture.token_failure.store(1, Ordering::SeqCst);
    assert!(part_transfer::upload(&store, KEY, BYTES.to_vec())
        .await
        .is_err());
    assert!(fixture.calls.lock().unwrap().is_empty());
    assert_eq!(fixture.tokens.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn owner_cancellation_during_request_or_backoff_stops_part_work() {
    for during_read in [true, false] {
        let (store, fixture, server) = fixture().await;
        let gate = Arc::new(Semaphore::new(0));
        if during_read {
            fixture
                .read_gates
                .lock()
                .unwrap()
                .insert(KEY.into(), gate.clone());
        } else {
            let mut fault = Fault::status(StatusCode::TOO_MANY_REQUESTS);
            fault.retry_after = Some("18446744073709551615");
            faults(&fixture, "POST", KEY, vec![fault]);
        }
        let uploading =
            tokio::spawn(async move { part_transfer::upload(&store, KEY, BYTES.to_vec()).await });
        wait_for_calls(&fixture, if during_read { 2 } else { 1 }).await;
        uploading.abort();
        assert!(uploading.await.unwrap_err().is_cancelled());
        let before = methods(&fixture, KEY);
        gate.add_permits(1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(methods(&fixture, KEY), before);
        server.abort();
    }
}

#[tokio::test]
async fn transient_late_part_and_manifest_failures_preserve_early_parts_and_order() {
    let (store, fixture, server) = fixture().await;
    let request = multipart_archive(1);
    let sha = request.sha256.clone();
    let key = checkpoint_object_key("cma/checkpoints/retry-late.cma".into(), request.size_bytes);
    let late = multipart::part_key(&key, 1);
    faults(
        &fixture,
        "POST",
        &late,
        vec![Fault::status(StatusCode::SERVICE_UNAVAILABLE)],
    );
    faults(
        &fixture,
        "GET",
        &key,
        vec![Fault::status(StatusCode::BAD_GATEWAY)],
    );
    store.upload(&key, request).await.unwrap();
    assert_eq!(
        methods(&fixture, &multipart::part_key(&key, 0)),
        ["POST", "GET"]
    );
    assert_eq!(methods(&fixture, &late), ["POST", "GET", "POST", "GET"]);
    assert_eq!(methods(&fixture, &key), ["POST", "GET", "GET"]);
    let objects = fixture.objects.lock().unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&objects[&key]).unwrap();
    assert_eq!(manifest["sha256"], sha);
    for index in 0..2 {
        assert_eq!(
            manifest["parts"][index as usize],
            format!(
                "{:x}",
                Sha256::digest(&objects[&multipart::part_key(&key, index)])
            )
        );
    }
    assert!(objects
        .values()
        .all(|part| part.len() <= multipart::PART_BYTES));
    server.abort();
}

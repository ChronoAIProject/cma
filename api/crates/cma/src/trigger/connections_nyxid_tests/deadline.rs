use std::{convert::Infallible, time::Duration};

use axum::body::Body;
use futures_util::{stream, StreamExt};
use tokio::{sync::Barrier, time::Instant};

use super::*;

const CALLER_BUDGET: Duration = Duration::from_secs(15);
const TEST_TOLERANCE: Duration = Duration::from_secs(2);
const SERVICES: &str = "/api/v1/user-services";
const GITHUB_KEY: &str = "/api/v1/keys/github-row";
const SECOND_KEY: &str = "/api/v1/keys/second-row";
const READINESS: &str = "/api/v1/assistant/readiness";

struct Timing {
    list_body_delay: Duration,
    second_key_body_delay: Duration,
    readiness_retry_after_secs: u64,
    readiness_retry_body_delay: Duration,
}

struct DeadlineProvider {
    timing: Timing,
    keys_started: Barrier,
    reads: Mutex<Vec<(&'static str, Instant)>>,
}

impl DeadlineProvider {
    fn observe_get(&self, route: &'static str, headers: &HeaderMap) -> usize {
        let credential = authorization(headers);
        assert!(
            credential
                .as_deref()
                .and_then(|value| value.strip_prefix("Bearer "))
                == Some(VISITOR_TOKEN)
        );
        let mut reads = self.reads.lock().unwrap();
        reads.push((route, Instant::now()));
        reads.iter().filter(|&&(read, _)| read == route).count()
    }
}

fn json_after_body_delay(value: Value, delay: Duration) -> Response {
    let tail = stream::once(async move {
        tokio::time::sleep(delay).await;
        Ok::<_, Infallible>(Bytes::from(value.to_string()))
    });
    // A first chunk lets the provider send successful headers before the
    // delayed JSON evidence, so the deadline must cover consuming the body.
    let body = stream::iter([Ok::<_, Infallible>(Bytes::from_static(b" "))]).chain(tail);
    (
        [(header::CONTENT_TYPE, "application/json")],
        Body::from_stream(body),
    )
        .into_response()
}

async fn list(State(provider): State<Arc<DeadlineProvider>>, headers: HeaderMap) -> Response {
    provider.observe_get(SERVICES, &headers);
    json_after_body_delay(
        json!({"services":[
            {"id":"github-row","slug":"api-github","is_active":true},
            {"id":"second-row","slug":"fixture-service","is_active":true}
        ]}),
        provider.timing.list_body_delay,
    )
}

async fn detail(
    State(provider): State<Arc<DeadlineProvider>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (route, slug) = match id.as_str() {
        "github-row" => (GITHUB_KEY, "api-github"),
        "second-row" => (SECOND_KEY, "fixture-service"),
        _ => panic!("unexpected credential lookup"),
    };
    assert_eq!(provider.observe_get(route, &headers), 1);
    // Neither detail request can complete until both reached the provider.
    provider.keys_started.wait().await;
    let evidence = json!({
        "id":id,"slug":slug,"is_active":true,"status":"active",
        "connected":true,"credential_missing":false
    });
    if route == SECOND_KEY {
        json_after_body_delay(evidence, provider.timing.second_key_body_delay)
    } else {
        Reply::json(evidence).response()
    }
}

async fn readiness(State(provider): State<Arc<DeadlineProvider>>, headers: HeaderMap) -> Response {
    if provider.observe_get(READINESS, &headers) == 1 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                header::RETRY_AFTER,
                provider.timing.readiness_retry_after_secs.to_string(),
            )],
        )
            .into_response();
    }
    json_after_body_delay(
        json!({"revision":"nyxid-assistant-readiness.v1","capabilities":[
            {"capabilityId":"api-github","connectionState":"connected"}
        ]}),
        provider.timing.readiness_retry_body_delay,
    )
}

async fn measure(timing: Timing) -> (Result<(), ServiceConnectionError>, Duration, usize) {
    let provider = Arc::new(DeadlineProvider {
        timing,
        keys_started: Barrier::new(2),
        reads: Mutex::default(),
    });
    let app = Router::new()
        .route(SERVICES, get(list))
        .route("/api/v1/keys/{id}", get(detail))
        .route(READINESS, get(readiness))
        .with_state(provider.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let broker = NyxidServiceConnectionBroker::new(
        reqwest::Client::builder()
            .timeout(CALLER_BUDGET)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        base_url,
        "https://nyx.example",
        Tokens::available(),
    )
    .unwrap();
    let owner = SubjectId::new("fixture-owner");
    let services = ["api-github", "fixture-service"].map(ServiceId::new);
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(24),
        broker.requirements(&owner, &services),
    )
    .await;
    let elapsed = started.elapsed();
    task.abort();
    let _ = task.await;

    // The outer timeout only bounds a broken test. A broker-owned refusal is
    // required below, so this timeout cannot satisfy the deadline assertions.
    let result = result.expect("broker exceeded the outer test watchdog");
    let reads = provider.reads.lock().unwrap();
    assert_eq!(reads.first().map(|&(route, _)| route), Some(SERVICES));
    for route in [SERVICES, GITHUB_KEY, SECOND_KEY] {
        assert_eq!(reads.iter().filter(|&&(read, _)| read == route).count(), 1);
    }
    let readiness_reads: Vec<_> = reads
        .iter()
        .filter_map(|&(route, at)| (route == READINESS).then_some(at))
        .collect();
    if let [first, retry, ..] = readiness_reads.as_slice() {
        assert!(
            retry.duration_since(*first)
                >= Duration::from_secs(provider.timing.readiness_retry_after_secs),
            "wire retry did not honor the provider floor"
        );
    }
    (result.map(|_| ()), elapsed, readiness_reads.len())
}

#[tokio::test]
async fn requirements_caller_deadline_refuses_retry_after_the_list_spends_its_budget() {
    let (result, elapsed, readiness_reads) = measure(Timing {
        list_body_delay: Duration::from_secs(12),
        second_key_body_delay: Duration::ZERO,
        readiness_retry_after_secs: 6,
        readiness_retry_body_delay: Duration::ZERO,
    })
    .await;

    assert_eq!(
        readiness_reads, 1,
        "the 6s provider floor cannot fit after a 12s listing in a 15s call; elapsed {elapsed:?}"
    );
    assert!(matches!(
        result,
        Err(ServiceConnectionError::Unavailable(_))
    ));
    assert!(elapsed <= CALLER_BUDGET + TEST_TOLERANCE);
}

#[tokio::test]
async fn requirements_caller_deadline_covers_parallel_and_retry_bodies() {
    let (result, elapsed, readiness_reads) = measure(Timing {
        list_body_delay: Duration::from_secs(8),
        second_key_body_delay: Duration::from_secs(10),
        readiness_retry_after_secs: 1,
        readiness_retry_body_delay: Duration::from_secs(9),
    })
    .await;

    assert_eq!(readiness_reads, 2, "the 1s provider floor must fit");
    assert!(
        matches!(result, Err(ServiceConnectionError::Unavailable(_))),
        "body evidence completed beyond the original deadline; elapsed {elapsed:?}"
    );
    assert!(elapsed >= CALLER_BUDGET - TEST_TOLERANCE);
    assert!(elapsed <= CALLER_BUDGET + TEST_TOLERANCE);
}

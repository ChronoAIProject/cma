use super::*;
use axum::{body::to_bytes, body::Body, http::Request, Router};
use std::{
    sync::{atomic::AtomicUsize, atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::sync::Barrier;
use tower::ServiceExt;

pub(super) static DOCUMENT_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub(super) static AGENT_MANAGER_BUILDS: AtomicUsize = AtomicUsize::new(0);
const PATHS: [&str; 2] = ["/openapi.json", "/agent-manager/openapi.json"];

fn request(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

async fn bytes(response: Response) -> Bytes {
    to_bytes(response.into_body(), usize::MAX).await.unwrap()
}

fn fresh_bytes(index: usize) -> Vec<u8> {
    if index == 0 {
        serde_json::to_vec(&openapi::document()).unwrap()
    } else {
        serde_json::to_vec(&openapi::agent_manager_document()).unwrap()
    }
}

#[tokio::test]
async fn cached_routes_preserve_bytes_and_independent_builds_are_deterministic() {
    let router = crate::rest::router();
    for (index, path) in PATHS.into_iter().enumerate() {
        let expected = fresh_bytes(index);
        assert_eq!(expected, fresh_bytes(index), "builder is deterministic");
        let first = router.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(first.headers()[header::CACHE_CONTROL], "no-cache");
        let etag = first.headers()[header::ETAG].clone();
        assert_eq!(etag, format!("\"{:x}\"", Sha256::digest(&expected)));
        let first = bytes(first).await;
        assert_eq!(first.as_ref(), expected);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&first).unwrap(),
            serde_json::from_slice::<serde_json::Value>(&expected).unwrap()
        );
        let second = router.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(second.headers()[header::ETAG], etag);
        assert_eq!(bytes(second).await, first);
    }
    assert_eq!(DOCUMENT_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(AGENT_MANAGER_BUILDS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn conditional_requests_require_one_exact_validator_and_head_has_no_body() {
    let router = crate::rest::router();
    for path in PATHS {
        let response = router.clone().oneshot(request(path)).await.unwrap();
        let etag = response.headers()[header::ETAG].clone();
        let expected = bytes(response).await;
        for value in [etag.clone(), HeaderValue::from_static("*")] {
            let mut req = request(path);
            req.headers_mut().insert(header::IF_NONE_MATCH, value);
            let response = router.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            assert_eq!(response.headers()[header::ETAG], etag);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            assert!(bytes(response).await.is_empty());
        }
        for value in [
            "\"other\"".to_string(),
            "malformed".to_string(),
            format!("W/{}", etag.to_str().unwrap()),
            format!("{}, \"other\"", etag.to_str().unwrap()),
            "".to_string(),
        ] {
            let mut req = request(path);
            req.headers_mut()
                .insert(header::IF_NONE_MATCH, value.parse().unwrap());
            let response = router.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(bytes(response).await, expected);
        }
        let mut req = request(path);
        req.headers_mut()
            .append(header::IF_NONE_MATCH, etag.clone());
        req.headers_mut()
            .append(header::IF_NONE_MATCH, etag.clone());
        let response = router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(bytes(response).await, expected);
        for validator in [None, Some(etag.clone())] {
            let mut req = Request::head(path).body(Body::empty()).unwrap();
            let expected_status = if let Some(value) = validator {
                req.headers_mut().insert(header::IF_NONE_MATCH, value);
                StatusCode::NOT_MODIFIED
            } else {
                StatusCode::OK
            };
            let response = router.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), expected_status);
            assert_eq!(response.headers()[header::ETAG], etag);
            assert!(bytes(response).await.is_empty());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_requests_and_second_requests_build_each_document_once() {
    // Nextest runs this test in its own process, exercising both cold statics.
    // The assertion also remains valid when cargo test shares process caches.
    let barrier = Arc::new(Barrier::new(65));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            let router = crate::rest::router();
            let path = PATHS[index % 2];
            let first = router.clone().oneshot(request(path)).await.unwrap();
            let second = router.oneshot(request(path)).await.unwrap();
            assert_eq!(first.status(), StatusCode::OK);
            assert_eq!(
                first.headers()[header::ETAG],
                second.headers()[header::ETAG]
            );
            assert_eq!(bytes(first).await, bytes(second).await);
        });
    }
    barrier.wait().await;
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(DOCUMENT_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(AGENT_MANAGER_BUILDS.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warmed_document_burst_keeps_no_io_route_responsive() {
    tokio::task::spawn_blocking(warm).await.unwrap();
    let router: Router = crate::rest::router();
    let barrier = Arc::new(Barrier::new(66));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let barrier = barrier.clone();
        let router = router.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            assert_eq!(
                router
                    .oneshot(request(PATHS[index % 2]))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        });
    }
    let probe_barrier = barrier.clone();
    let probe = tokio::spawn(async move {
        probe_barrier.wait().await;
        router
            .oneshot(request("/api/v1/conformance"))
            .await
            .unwrap()
    });
    let start = Instant::now();
    barrier.wait().await;
    assert_eq!(probe.await.unwrap().status(), StatusCode::OK);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(50),
        "probe took {elapsed:?}"
    );
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

#[tokio::test]
async fn report_uncached_build_cost_and_warmed_request_timing() {
    for (index, path) in PATHS.into_iter().enumerate() {
        let start = Instant::now();
        let bytes = fresh_bytes(index);
        eprintln!(
            "{path} uncached_build_and_serialize={:?} bytes={}",
            start.elapsed(),
            bytes.len()
        );
    }
    let start = Instant::now();
    tokio::task::spawn_blocking(warm).await.unwrap();
    eprintln!("startup_warm={:?}", start.elapsed());
    let router = crate::rest::router();
    for path in PATHS {
        let mut elapsed = Vec::new();
        for _ in 0..256 {
            let start = Instant::now();
            let response = router.clone().oneshot(request(path)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let _body = bytes(response).await;
            elapsed.push(start.elapsed());
        }
        elapsed.sort_unstable();
        eprintln!("{path} warmed_request_p99={:?}", elapsed[253]);
        assert!(elapsed[253] < Duration::from_millis(5));
    }
}

#[test]
fn serialization_failure_cannot_produce_a_cache() {
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("broken document fixture"))
        }
    }
    assert!(std::panic::catch_unwind(|| CachedDocument::new(&Broken)).is_err());
}

use super::*;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, Response as HttpResponse},
    routing::{get, post},
    Form, Json, Router,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Probe {
    issued: AtomicUsize,
    rejected: AtomicUsize,
    bytes: Mutex<Vec<u8>>,
    mode: AtomicUsize,
}
async fn auth(
    State(state): State<Arc<Probe>>,
    Form(input): Form<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    assert_eq!(
        input.get("grant_type").map(String::as_str),
        Some("client_credentials")
    );
    assert_eq!(
        input.get("client_id").map(String::as_str),
        Some("fixture-machine")
    );
    assert_eq!(
        input.get("client_secret").map(String::as_str),
        Some("local-fixture")
    );
    assert_eq!(
        input.get("scope").map(String::as_str),
        Some("proxy object:create object:read object:download")
    );
    let count = state.issued.fetch_add(1, Ordering::SeqCst) + 1;
    Json(serde_json::json!({"access_token":format!("test-authority-{count}"),"expires_in":3600}))
}
async fn write(
    State(state): State<Arc<Probe>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> HttpResponse<Body> {
    if state.mode.load(Ordering::SeqCst) == 1
        && headers.get("authorization").and_then(|v| v.to_str().ok())
            == Some("Bearer test-authority-1")
    {
        state.rejected.fetch_add(1, Ordering::SeqCst);
        return HttpResponse::builder()
            .status(401)
            .body(Body::empty())
            .unwrap();
    }
    if query.get("mutationGeneration").map(String::as_str) != Some("1")
        || query.get("contentSha256") != Some(&digest(&body))
    {
        return HttpResponse::builder()
            .status(400)
            .body(Body::empty())
            .unwrap();
    }
    *state.bytes.lock().await = body.to_vec();
    let result = serde_json::json!({"data":{"key":query["key"],"contentLength":body.len(),"generation":1,
        "digest": if state.mode.load(Ordering::SeqCst) == 2 { "wrong".into() } else { digest(&body) },"fenceVersion":1}});
    HttpResponse::builder()
        .header("content-type", "application/json")
        .body(Body::from(result.to_string()))
        .unwrap()
}
async fn read(State(state): State<Arc<Probe>>) -> HttpResponse<Body> {
    match state.mode.load(Ordering::SeqCst) {
        3 => HttpResponse::builder()
            .status(302)
            .header("location", "/untrusted")
            .body(Body::empty())
            .unwrap(),
        4 => HttpResponse::builder()
            .body(Body::from(vec![0; MAX_DOCUMENT_BYTES + 1]))
            .unwrap(),
        _ => HttpResponse::new(Body::from(state.bytes.lock().await.clone())),
    }
}
async fn fixture() -> (
    Arc<StorageDocuments>,
    Arc<Probe>,
    tokio::task::JoinHandle<()>,
) {
    let state = Arc::new(Probe::default());
    let app = Router::new()
        .route("/oauth/token", post(auth))
        .route(
            "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects",
            post(write),
        )
        .route(
            "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects/download",
            get(read),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = StorageDocuments::new(DocumentStorageConfig {
        platform_origin: origin,
        bucket: "fixture".into(),
        client_id: "fixture-machine".into(),
        client_secret: "local-fixture".to_owned().into(),
    })
    .unwrap();
    (client, state, task)
}
fn key(bytes: &[u8]) -> String {
    format!(
        "documents/{}/doc_{}/{}.md",
        digest(b"owner"),
        "a".repeat(32),
        digest(bytes)
    )
}
#[tokio::test]
async fn exact_fenced_upload_download_and_single_flight_token_refresh() {
    let (client, probe, task) = fixture().await;
    probe.mode.store(1, Ordering::SeqCst);
    let data = b"# Durable document\n".to_vec();
    let path = key(&data);
    client.put(&path, data.clone()).await.unwrap();
    assert_eq!(client.get(&path).await.unwrap(), data);
    assert_eq!(probe.issued.load(Ordering::SeqCst), 2);
    assert_eq!(probe.rejected.load(Ordering::SeqCst), 1);
    let (a, b) = tokio::join!(client.get(&path), client.get(&path));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(probe.issued.load(Ordering::SeqCst), 2);
    task.abort();
}
#[tokio::test]
async fn rejects_false_write_receipts_redirects_and_oversized_downloads() {
    let (client, probe, task) = fixture().await;
    let data = b"# Durable".to_vec();
    let path = key(&data);
    probe.mode.store(2, Ordering::SeqCst);
    assert_eq!(client.put(&path, data).await, Err(DocumentError::Corrupt));
    probe.mode.store(3, Ordering::SeqCst);
    assert_eq!(client.get(&path).await, Err(DocumentError::Unavailable));
    probe.mode.store(4, Ordering::SeqCst);
    assert_eq!(client.get(&path).await, Err(DocumentError::Corrupt));
    task.abort();
}
#[tokio::test]
async fn refuses_caller_paths_and_wrong_content_without_contacting_storage() {
    let (client, probe, task) = fixture().await;
    for path in [
        "https://example.invalid/private",
        "../other",
        "documents/owner/../../secret",
        "documents/%2f",
    ] {
        assert_eq!(client.get(path).await, Err(DocumentError::Invalid));
    }
    assert_eq!(
        client.put(&key(b"expected"), b"different".to_vec()).await,
        Err(DocumentError::Invalid)
    );
    assert_eq!(probe.issued.load(Ordering::SeqCst), 0);
    task.abort();
}

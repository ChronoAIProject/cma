use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Notify, Semaphore};

use super::*;

mod deletion_fence;
mod part_deletion;
mod part_download;
mod part_recovery;

const OBJECTS: &str = "/api/v1/proxy/s/chrono-storage-service/api/buckets/checkpoint-test/objects";

struct Fault {
    status: StatusCode,
    retry_after: Option<&'static str>,
    commit: bool,
    disconnect: bool,
    chunked: bool,
    body: Option<Vec<u8>>,
}

impl Fault {
    fn status(status: StatusCode) -> Self {
        Self {
            status,
            retry_after: None,
            commit: false,
            disconnect: false,
            chunked: false,
            body: None,
        }
    }

    fn response(self) -> Response {
        let mut response = if self.disconnect {
            let stream = futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::other("fixture interrupted response"))
            });
            (self.status, axum::body::Body::from_stream(stream)).into_response()
        } else if self.chunked {
            let stream = futures_util::stream::iter(
                self.body
                    .unwrap_or_default()
                    .chunks(7)
                    .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
                    .collect::<Vec<_>>(),
            );
            (self.status, axum::body::Body::from_stream(stream)).into_response()
        } else {
            (self.status, self.body.unwrap_or_default()).into_response()
        };
        if let Some(value) = self.retry_after {
            response
                .headers_mut()
                .insert("retry-after", value.parse().unwrap());
        }
        response
    }
}

#[derive(Default)]
struct Fixture {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    calls: Mutex<Vec<(String, String)>>,
    tokens: AtomicUsize,
    token_failure: AtomicUsize,
    corrupt: AtomicBool,
    ignore_delete: AtomicBool,
    redirect: AtomicBool,
    read_gates: Mutex<BTreeMap<String, Arc<Semaphore>>>,
    faults: Mutex<BTreeMap<(String, String), VecDeque<Fault>>>,
    requests_changed: Notify,
    fenced_deletes: AtomicBool,
    deleted_keys: Mutex<std::collections::BTreeSet<String>>,
}

impl Fixture {
    fn fault(&self, method: &str, key: &str) -> Option<Fault> {
        self.faults
            .lock()
            .unwrap()
            .get_mut(&(method.into(), key.into()))
            .and_then(VecDeque::pop_front)
    }
}

async fn mint(
    State(fixture): State<Arc<Fixture>>,
    Form(fields): Form<BTreeMap<String, String>>,
) -> Response {
    assert_eq!(fields["grant_type"], "client_credentials");
    assert_eq!(fields["client_id"], "machine-client");
    assert_eq!(fields["client_secret"], "fixture-material");
    fixture.tokens.fetch_add(1, Ordering::SeqCst);
    match fixture.token_failure.load(Ordering::SeqCst) {
        1 => (
            StatusCode::UNAUTHORIZED,
            "fixture-material fixture-storage-bearer",
        )
            .into_response(),
        2 => (StatusCode::TEMPORARY_REDIRECT, [("location", "/forbidden")]).into_response(),
        3 => Json(serde_json::json!({ "access_token": "x".repeat(40_000), "expires_in": 900 }))
            .into_response(),
        4 => {
            Json(serde_json::json!({ "access_token": "fixture-storage-bearer", "expires_in": 60 }))
                .into_response()
        }
        5 => Json(
            serde_json::json!({ "access_token": "invalid\nfixture-material", "expires_in": 900 }),
        )
        .into_response(),
        _ => {
            Json(serde_json::json!({ "access_token": "fixture-storage-bearer", "expires_in": 900 }))
                .into_response()
        }
    }
}

fn authorized(headers: &HeaderMap) {
    assert_eq!(headers["authorization"], "Bearer fixture-storage-bearer");
}

async fn upload(
    State(fixture): State<Arc<Fixture>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    authorized(&headers);
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(
        query["contentType"],
        "application/vnd.chrono.cma-checkpoint.v1"
    );
    fixture
        .calls
        .lock()
        .unwrap()
        .push(("POST".into(), query["key"].clone()));
    fixture.requests_changed.notify_one();
    if fixture.deleted_keys.lock().unwrap().contains(&query["key"]) {
        return StatusCode::CONFLICT.into_response();
    }
    if fixture.redirect.load(Ordering::SeqCst) {
        return (StatusCode::TEMPORARY_REDIRECT, [("location", "/forbidden")]).into_response();
    }
    if let Some(fault) = fixture.fault("POST", &query["key"]) {
        if fault.commit {
            fixture
                .objects
                .lock()
                .unwrap()
                .insert(query["key"].clone(), body.to_vec());
        }
        return fault.response();
    }
    fixture
        .objects
        .lock()
        .unwrap()
        .insert(query["key"].clone(), body.to_vec());
    StatusCode::CREATED.into_response()
}

async fn download(
    State(fixture): State<Arc<Fixture>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    authorized(&headers);
    fixture
        .calls
        .lock()
        .unwrap()
        .push(("GET".into(), query["key"].clone()));
    fixture.requests_changed.notify_one();
    let gate = fixture
        .read_gates
        .lock()
        .unwrap()
        .get(&query["key"])
        .cloned();
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    if let Some(fault) = fixture.fault("GET", &query["key"]) {
        return fault.response();
    }
    match fixture.objects.lock().unwrap().get(&query["key"]).cloned() {
        Some(mut bytes) => {
            if fixture.corrupt.load(Ordering::SeqCst) && !bytes.is_empty() {
                bytes[0] ^= 1;
            }
            bytes.into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn delete(
    State(fixture): State<Arc<Fixture>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    authorized(&headers);
    fixture
        .calls
        .lock()
        .unwrap()
        .push(("DELETE".into(), query["key"].clone()));
    if let Some(fault) = fixture.fault("DELETE", &query["key"]) {
        return fault.response();
    }
    if fixture.fenced_deletes.load(Ordering::SeqCst) {
        assert_eq!(query["mutationGeneration"], "2");
        fixture
            .deleted_keys
            .lock()
            .unwrap()
            .insert(query["key"].clone());
        fixture.objects.lock().unwrap().remove(&query["key"]);
        return Json(serde_json::json!({"data":{"deleted":true,"generation":2,"fenceVersion":1},"error":null})).into_response();
    }
    if !fixture.ignore_delete.load(Ordering::SeqCst) {
        fixture.objects.lock().unwrap().remove(&query["key"]);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn fixture() -> (ChronoStorage, Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let fixture = Arc::new(Fixture::default());
    let app = Router::new()
        .route("/oauth/token", post(mint))
        .route(OBJECTS, post(upload).delete(delete))
        .route(&format!("{OBJECTS}/download"), get(download))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin =
        reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let store = ChronoStorage::from_origin(
        origin,
        &CheckpointStorageConfig {
            bucket: "checkpoint-test".into(),
            client_id: "machine-client".into(),
            client_secret: "fixture-material".into(),
        },
    )
    .unwrap();
    (store, fixture, server)
}

fn archive(bytes: &[u8]) -> CheckpointArchiveUpload {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let bytes = bytes.to_vec();
    let size_bytes = bytes.len() as u64;
    let (sender, chunks) = mpsc::channel(2);
    tokio::spawn(async move {
        for chunk in bytes.chunks(64 * 1024) {
            if sender.send(Ok(chunk.to_vec())).await.is_err() {
                break;
            }
        }
    });
    CheckpointArchiveUpload {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        size_bytes,
        chunks,
    }
}

async fn consume(mut download: CheckpointArchiveDownload) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = download.chunks.recv().await {
        bytes.extend(chunk?);
    }
    Ok(bytes)
}

fn multipart_archive(full_parts: usize) -> CheckpointArchiveUpload {
    let size_bytes = (full_parts * multipart::PART_BYTES + 4096) as u64;
    let mut digest = Sha256::new();
    for index in 0..=full_parts {
        let block = vec![index as u8; 64 * 1024];
        let size = if index == full_parts {
            4096
        } else {
            multipart::PART_BYTES
        };
        for offset in (0..size).step_by(block.len()) {
            digest.update(&block[..(size - offset).min(block.len())]);
        }
    }
    let sha256 = format!("{:x}", digest.finalize());
    let (sender, chunks) = mpsc::channel(2);
    tokio::spawn(async move {
        for index in 0..=full_parts {
            let size = if index == full_parts {
                4096
            } else {
                multipart::PART_BYTES
            };
            for offset in (0..size).step_by(64 * 1024) {
                let chunk = vec![index as u8; (size - offset).min(64 * 1024)];
                if sender.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
        }
    });
    CheckpointArchiveUpload {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        size_bytes,
        chunks,
    }
}

async fn wait_for_part_reads(fixture: &Fixture, key: &str, count: usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let changed = fixture.requests_changed.notified();
            let reads = fixture
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, target)| {
                    method == "GET" && target.starts_with(&format!("{key}/"))
                })
                .count();
            if reads >= count {
                return;
            }
            changed.await;
        }
    })
    .await
    .expect("expected concurrent part readbacks");
}

#[tokio::test]
async fn multipart_upload_bounds_concurrency_and_publishes_ordered_manifest_after_readbacks() {
    let (store, fixture, server) = fixture().await;
    let count = multipart::UPLOAD_CONCURRENCY + 1;
    let request = multipart_archive(count);
    let size = request.size_bytes;
    let sha = request.sha256.clone();
    let key = checkpoint_object_key("cma/checkpoints/concurrent.cma".into(), size);
    let gates = (0..=count)
        .map(|index| {
            let gate = Arc::new(Semaphore::new(0));
            fixture
                .read_gates
                .lock()
                .unwrap()
                .insert(multipart::part_key(&key, index as u64), gate.clone());
            gate
        })
        .collect::<Vec<_>>();
    let uploading = {
        let store = store.clone();
        let key = key.clone();
        tokio::spawn(async move { store.upload(&key, request).await })
    };
    wait_for_part_reads(&fixture, &key, multipart::UPLOAD_CONCURRENCY).await;
    {
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(
            calls.iter().filter(|(method, _)| method == "POST").count(),
            multipart::UPLOAD_CONCURRENCY
        );
        assert!(!calls.iter().any(|(_, target)| target == &key));
    }
    assert!(!fixture.objects.lock().unwrap().contains_key(&key));
    for gate in gates[..multipart::UPLOAD_CONCURRENCY].iter().rev() {
        gate.add_permits(1);
    }
    wait_for_part_reads(&fixture, &key, count + 1).await;
    assert!(!fixture.objects.lock().unwrap().contains_key(&key));
    for gate in gates[multipart::UPLOAD_CONCURRENCY..].iter().rev() {
        gate.add_permits(1);
    }
    uploading.await.unwrap().unwrap();
    {
        let objects = fixture.objects.lock().unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&objects[&key]).unwrap();
        assert_eq!(manifest["size_bytes"], size);
        assert_eq!(manifest["sha256"], sha);
        assert_eq!(manifest["parts"].as_array().unwrap().len(), count + 1);
        for index in 0..=count {
            let bytes = &objects[&multipart::part_key(&key, index as u64)];
            assert!(bytes.iter().all(|byte| *byte == index as u8));
            assert_eq!(
                manifest["parts"][index],
                format!("{:x}", Sha256::digest(bytes))
            );
        }
    }
    assert_eq!(
        fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>(),
        vec![("GET".into(), key.clone()), ("POST".into(), key.clone())]
    );
    assert_eq!(fixture.tokens.load(Ordering::SeqCst), 1);
    fixture.read_gates.lock().unwrap().clear();
    store.delete(&key).await.unwrap();
    assert!(fixture.objects.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn concurrent_part_readback_failure_never_publishes_a_manifest() {
    let (store, fixture, server) = fixture().await;
    fixture.corrupt.store(true, Ordering::SeqCst);
    let request = multipart_archive(2);
    let key = checkpoint_object_key("cma/checkpoints/failed-part.cma".into(), request.size_bytes);
    assert!(store.upload(&key, request).await.is_err());
    assert!(!fixture.objects.lock().unwrap().contains_key(&key));
    assert!(!fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(method, target)| method == "POST" && target == &key));
    fixture.corrupt.store(false, Ordering::SeqCst);
    store.delete(&key).await.unwrap();
    assert!(fixture.objects.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn historical_object_contract_streams_verifies_reuses_identity_and_deletes_exactly() {
    let (store, fixture, server) = fixture().await;
    let bytes = vec![0x35; 16 * 1_024 * 1_024 + 4096];
    let request = archive(&bytes);
    let reference = request.object_reference.clone();
    let digest = request.sha256.clone();
    store
        .upload("cma/checkpoints/owner/object.cma", request)
        .await
        .unwrap();
    assert_eq!(
        consume(
            store
                .download(
                    "cma/checkpoints/owner/object.cma",
                    &reference,
                    &digest,
                    bytes.len() as u64
                )
                .await
                .unwrap()
        )
        .await
        .unwrap(),
        bytes
    );
    store
        .delete("cma/checkpoints/owner/object.cma")
        .await
        .unwrap();
    store
        .delete("cma/checkpoints/owner/object.cma")
        .await
        .unwrap();
    assert_eq!(fixture.tokens.load(Ordering::SeqCst), 1);
    assert!(fixture.objects.lock().unwrap().is_empty());
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|(_, key)| key == "cma/checkpoints/owner/object.cma"));
    server.abort();
}

#[tokio::test]
async fn corrupt_readback_failed_deletion_and_redirects_are_not_acknowledged() {
    let (store, fixture, server) = fixture().await;
    fixture.corrupt.store(true, Ordering::SeqCst);
    assert!(store
        .upload("corrupt", archive(b"complete checkpoint"))
        .await
        .is_err());
    fixture.corrupt.store(false, Ordering::SeqCst);
    fixture.ignore_delete.store(true, Ordering::SeqCst);
    assert!(store.delete("corrupt").await.is_err());
    fixture.redirect.store(true, Ordering::SeqCst);
    assert!(store
        .upload("redirected", archive(b"complete checkpoint"))
        .await
        .is_err());
    assert!(!fixture.objects.lock().unwrap().contains_key("redirected"));
    server.abort();
}

#[tokio::test]
async fn upload_and_download_enforce_hash_and_exact_declared_size() {
    let (store, _fixture, server) = fixture().await;
    let mut request = archive(b"actual bytes");
    request.sha256 = "0".repeat(64);
    assert!(store.upload("wrong-digest", request).await.is_err());
    let mut request = archive(b"actual bytes");
    request.size_bytes += 1;
    assert!(store.upload("short-stream", request).await.is_err());
    let mut request = archive(b"actual bytes");
    request.size_bytes -= 1;
    assert!(store.upload("long-stream", request).await.is_err());
    let mut request = archive(b"actual bytes");
    let (sender, chunks) = mpsc::channel(2);
    sender.try_send(Ok(b"actual bytes".to_vec())).unwrap();
    sender.try_send(Ok(b"unexpected tail".to_vec())).unwrap();
    drop(sender);
    request.chunks = chunks;
    assert!(store.upload("trailing-chunk", request).await.is_err());
    assert!(store
        .download("missing", "reference", "digest", 1)
        .await
        .is_err());
    server.abort();
}

#[tokio::test]
async fn large_checkpoints_use_bounded_parts_and_restore_the_exact_archive() {
    let (store, fixture, server) = fixture().await;
    let bytes = vec![0x35; multipart::PART_BYTES + 4096];
    let request = archive(&bytes);
    let reference = request.object_reference.clone();
    let sha = request.sha256.clone();
    let key = checkpoint_object_key("cma/checkpoints/large.cma".into(), bytes.len() as u64);
    store.upload(&key, request).await.unwrap();
    {
        let objects = fixture.objects.lock().unwrap();
        assert_eq!(objects.len(), 3);
        assert!(objects
            .values()
            .all(|body| body.len() <= multipart::PART_BYTES));
    }
    let restored = consume(
        store
            .download(&key, &reference, &sha, bytes.len() as u64)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(restored, bytes);
    let first_key = multipart::part_key(&key, 0);
    fixture.objects.lock().unwrap().get_mut(&first_key).unwrap()[0] ^= 1;
    assert!(consume(
        store
            .download(&key, &reference, &sha, bytes.len() as u64)
            .await
            .unwrap()
    )
    .await
    .is_err());
    fixture.objects.lock().unwrap().remove(&first_key);
    assert!(consume(
        store
            .download(&key, &reference, &sha, bytes.len() as u64)
            .await
            .unwrap()
    )
    .await
    .is_err());
    store.delete(&key).await.unwrap();
    store.delete(&key).await.unwrap();
    assert!(fixture.objects.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn partial_large_upload_is_unpublished_and_cleanup_owns_every_part() {
    let (store, fixture, server) = fixture().await;
    let bytes = vec![0x35; multipart::PART_BYTES + 4096];
    let mut request = archive(&bytes);
    request.sha256 = "0".repeat(64);
    let key = checkpoint_object_key("cma/checkpoints/partial.cma".into(), bytes.len() as u64);
    assert!(store.upload(&key, request).await.is_err());
    assert!(!fixture.objects.lock().unwrap().contains_key(&key));
    assert!(fixture
        .objects
        .lock()
        .unwrap()
        .contains_key(&multipart::part_key(&key, 0)));
    fixture.ignore_delete.store(true, Ordering::SeqCst);
    assert!(store.delete(&key).await.is_err());
    fixture.ignore_delete.store(false, Ordering::SeqCst);
    store.delete(&key).await.unwrap();
    assert!(fixture.objects.lock().unwrap().is_empty());
    let calls = fixture.calls.lock().unwrap();
    for index in 0..2 {
        assert!(calls.contains(&("DELETE".into(), multipart::part_key(&key, index))));
    }
    server.abort();
}

#[tokio::test]
async fn multipart_manifest_cannot_redirect_reads_or_change_archive_identity() {
    let (store, fixture, server) = fixture().await;
    let size = multipart::PART_BYTES as u64 + 1;
    let key = checkpoint_object_key("cma/checkpoints/malformed.cma".into(), size);
    let sha = "0".repeat(64);
    for manifest in [
        serde_json::json!({"schema":"other","size_bytes":size,"sha256":sha,"parts":[sha,sha]}),
        serde_json::json!({"schema":"cma-checkpoint-parts-v1","size_bytes":size + 1,"sha256":sha,"parts":[sha,sha]}),
        serde_json::json!({"schema":"cma-checkpoint-parts-v1","size_bytes":size,"sha256":sha,"parts":[sha]}),
        serde_json::json!({"schema":"cma-checkpoint-parts-v1","size_bytes":size,"sha256":sha,"parts":["https://foreign.example",sha]}),
    ] {
        fixture
            .objects
            .lock()
            .unwrap()
            .insert(key.clone(), serde_json::to_vec(&manifest).unwrap());
        assert!(store.download(&key, "archive", &sha, size).await.is_err());
    }
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|(method, target)| method == "GET" && target == &key));
    server.abort();
}

#[tokio::test]
async fn failed_oversized_redirected_or_unusable_machine_tokens_are_redacted() {
    for mode in 1..=5 {
        let (store, fixture, server) = fixture().await;
        fixture.token_failure.store(mode, Ordering::SeqCst);
        let error = store
            .upload("private-storage-key", archive(b"checkpoint"))
            .await
            .unwrap_err();
        let rendered = format!("{error:?}");
        for secret in [
            "fixture-material",
            "fixture-storage-bearer",
            "private-storage-key",
            "127.0.0.1",
        ] {
            assert!(
                !rendered.contains(secret),
                "token failure {mode} leaked sensitive context"
            );
        }
        assert!(fixture.calls.lock().unwrap().is_empty());
        server.abort();
    }
}

#[test]
fn production_constructor_refuses_other_schemes_paths_and_authorities() {
    let config = CheckpointStorageConfig {
        bucket: "checkpoint-test".into(),
        client_id: "machine-client".into(),
        client_secret: "fixture-material".into(),
    };
    for origin in [
        "http://platform.example",
        "https://platform.example/",
        "https://platform.example/path",
        "https://user@platform.example",
        "https://platform.example?query",
    ] {
        assert!(ChronoStorage::new(origin, &config).is_err());
    }
    assert!(ChronoStorage::new("https://platform.example", &config).is_ok());
}

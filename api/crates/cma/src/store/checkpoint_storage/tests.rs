use std::collections::BTreeMap;
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
use tokio::sync::mpsc;

use super::*;

const OBJECTS: &str = "/api/v1/proxy/s/chrono-storage-service/api/buckets/checkpoint-test/objects";

#[derive(Default)]
struct Fixture {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    calls: Mutex<Vec<(String, String)>>,
    tokens: AtomicUsize,
    token_failure: AtomicUsize,
    corrupt: AtomicBool,
    ignore_delete: AtomicBool,
    redirect: AtomicBool,
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
    if fixture.redirect.load(Ordering::SeqCst) {
        return (StatusCode::TEMPORARY_REDIRECT, [("location", "/forbidden")]).into_response();
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
) -> StatusCode {
    authorized(&headers);
    fixture
        .calls
        .lock()
        .unwrap()
        .push(("DELETE".into(), query["key"].clone()));
    if !fixture.ignore_delete.load(Ordering::SeqCst) {
        fixture.objects.lock().unwrap().remove(&query["key"]);
    }
    StatusCode::NO_CONTENT
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

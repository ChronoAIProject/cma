use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::*;
use crate::hosted::{CheckpointArchiveReceipt, CheckpointArchiveStore};
use crate::WriterState;

const AUTH: (&str, &str) = ("OPEN-SANDBOX-API-KEY", "provider-secret");

struct HttpFixture {
    url: Url,
    request: tokio::task::JoinHandle<Vec<u8>>,
}

async fn serve(response: Vec<u8>, path: &str) -> HttpFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP fixture binds");
    let address = listener.local_addr().expect("HTTP fixture address");
    let request = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("HTTP fixture accepts");
        let request = read_request(&mut stream).await;
        stream
            .write_all(&response)
            .await
            .expect("HTTP fixture writes response");
        stream.shutdown().await.expect("HTTP fixture closes");
        request
    });
    HttpFixture {
        url: Url::parse(&format!("http://{address}{path}")).expect("fixture URL"),
        request,
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await.expect("fixture reads");
        assert!(read > 0, "client closed before request headers completed");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or_default();
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await.expect("fixture reads body");
        assert!(read > 0, "client closed before request body completed");
        bytes.extend_from_slice(&chunk[..read]);
    }
    bytes
}

fn response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let mut bytes = response.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test HTTP client")
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn checkpoint(bytes: &[u8]) -> CheckpointReceipt {
    let sha256 = digest(bytes);
    CheckpointReceipt {
        epoch: 7,
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        writers: WriterState::Resumed,
    }
}

fn restore(bytes: &[u8]) -> RestoreRequest {
    let sha256 = digest(bytes);
    RestoreRequest {
        object_reference: format!("home-{sha256}.cma"),
        sha256,
    }
}

#[derive(Default)]
struct UploadStore {
    calls: AtomicUsize,
    uploaded: Mutex<Option<UploadedArchive>>,
    alter_receipt: bool,
    refuse_upload: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct UploadedArchive {
    object_reference: String,
    sha256: String,
    size_bytes: u64,
    bytes: Vec<u8>,
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for UploadStore {
    async fn upload(
        &self,
        mut archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse_upload {
            return Err(AdapterError::Unavailable(
                "fixture storage rejection".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = archive.chunks.recv().await {
            bytes.extend_from_slice(&chunk?);
        }
        *self.uploaded.lock().expect("upload record lock") = Some(UploadedArchive {
            object_reference: archive.object_reference.clone(),
            sha256: archive.sha256.clone(),
            size_bytes: archive.size_bytes,
            bytes,
        });
        Ok(CheckpointArchiveReceipt {
            object_reference: archive.object_reference,
            sha256: archive.sha256,
            size_bytes: archive.size_bytes + u64::from(self.alter_receipt),
        })
    }

    async fn download(&self, _object_reference: &str) -> crate::Result<CheckpointArchiveDownload> {
        unreachable!("upload store is not downloaded")
    }
}

struct DownloadStore {
    reference: String,
    sha256: String,
    size_bytes: u64,
    chunks: Mutex<Option<Vec<crate::Result<Vec<u8>>>>>,
}

impl DownloadStore {
    fn new(request: &RestoreRequest, declared: u64, chunks: Vec<crate::Result<Vec<u8>>>) -> Self {
        Self {
            reference: request.object_reference.clone(),
            sha256: request.sha256.clone(),
            size_bytes: declared,
            chunks: Mutex::new(Some(chunks)),
        }
    }
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for DownloadStore {
    async fn upload(
        &self,
        _archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt> {
        unreachable!("download store is not uploaded")
    }

    async fn download(&self, _object_reference: &str) -> crate::Result<CheckpointArchiveDownload> {
        let chunks = self
            .chunks
            .lock()
            .expect("download chunks lock")
            .take()
            .expect("download happens once");
        let (sender, receiver) = mpsc::channel(chunks.len().max(1));
        for chunk in chunks {
            sender.try_send(chunk).expect("download channel capacity");
        }
        drop(sender);
        Ok(CheckpointArchiveDownload {
            object_reference: self.reference.clone(),
            sha256: self.sha256.clone(),
            size_bytes: self.size_bytes,
            chunks: receiver,
        })
    }
}

#[tokio::test]
async fn export_streams_the_exact_runtime_archive_before_acknowledging() {
    let bytes = b"sealed checkpoint bytes";
    let checkpoint = checkpoint(bytes);
    let fixture = serve(
        response(
            "200 OK",
            &[("Content-Length", bytes.len().to_string())],
            bytes,
        ),
        "/proxy/runtime/checkpoints/archive",
    )
    .await;
    let store = Arc::new(UploadStore::default());
    let erased: Arc<dyn CheckpointArchiveStore> = store.clone();

    export(&client(), fixture.url, AUTH, &checkpoint, &erased)
        .await
        .expect("valid export commits");

    let request = String::from_utf8(fixture.request.await.expect("fixture completes"))
        .expect("export request is text");
    assert!(request.starts_with("GET /proxy/runtime/checkpoints/archive HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    assert_eq!(
        store.uploaded.lock().expect("upload record lock").as_ref(),
        Some(&UploadedArchive {
            object_reference: checkpoint.object_reference,
            sha256: checkpoint.sha256,
            size_bytes: bytes.len() as u64,
            bytes: bytes.to_vec(),
        })
    );
}

#[tokio::test]
async fn export_rejects_missing_zero_or_oversized_content_lengths() {
    let checkpoint = checkpoint(b"x");
    let store = Arc::new(UploadStore::default());
    let erased: Arc<dyn CheckpointArchiveStore> = store.clone();
    let cases = [
        Vec::new(),
        vec![("Content-Length", "0".to_string())],
        vec![(
            "Content-Length",
            (MAX_CHECKPOINT_ARCHIVE_BYTES + 1).to_string(),
        )],
    ];

    for headers in cases {
        let fixture = serve(response("200 OK", &headers, &[]), "/checkpoint").await;
        let error = export(&client(), fixture.url, AUTH, &checkpoint, &erased)
            .await
            .expect_err("invalid content length is refused");
        assert!(matches!(error, AdapterError::Rejected(_)));
        fixture.request.await.expect("fixture completes");
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn export_preserves_storage_rejection_when_the_upload_channel_closes() {
    let bytes = vec![0x35; 128 * 1024];
    let checkpoint = checkpoint(&bytes);
    let fixture = serve(
        response(
            "200 OK",
            &[("Content-Length", bytes.len().to_string())],
            &bytes,
        ),
        "/checkpoint",
    )
    .await;
    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(UploadStore {
        refuse_upload: true,
        ..UploadStore::default()
    });
    assert_eq!(
        export(&client(), fixture.url, AUTH, &checkpoint, &store)
            .await
            .unwrap_err(),
        AdapterError::Unavailable("fixture storage rejection".into())
    );
    let _ = fixture.request.await;
}

#[tokio::test]
async fn export_rejects_stream_hash_and_durable_receipt_mismatches() {
    let bytes = b"runtime bytes";
    let wrong_checkpoint = checkpoint(b"different bytes");
    let fixture = serve(
        response(
            "200 OK",
            &[("Content-Length", bytes.len().to_string())],
            bytes,
        ),
        "/checkpoint",
    )
    .await;
    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(UploadStore::default());
    let error = export(&client(), fixture.url, AUTH, &wrong_checkpoint, &store)
        .await
        .expect_err("runtime hash mismatch is refused");
    assert!(matches!(error, AdapterError::Rejected(_)));
    fixture.request.await.expect("fixture completes");

    let checkpoint = checkpoint(bytes);
    let fixture = serve(
        response(
            "200 OK",
            &[("Content-Length", bytes.len().to_string())],
            bytes,
        ),
        "/checkpoint",
    )
    .await;
    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(UploadStore {
        alter_receipt: true,
        ..UploadStore::default()
    });
    let error = export(&client(), fixture.url, AUTH, &checkpoint, &store)
        .await
        .expect_err("durable receipt mismatch is refused");
    assert!(matches!(error, AdapterError::Rejected(_)));
    fixture.request.await.expect("fixture completes");
}

#[tokio::test]
async fn import_streams_exact_length_and_requires_no_content() {
    let bytes = b"durable checkpoint bytes";
    let request = restore(bytes);
    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(DownloadStore::new(
        &request,
        bytes.len() as u64,
        vec![Ok(bytes[..8].to_vec()), Ok(bytes[8..].to_vec())],
    ));
    let fixture = serve(
        response("204 No Content", &[("Content-Length", "0".into())], &[]),
        "/proxy/runtime/checkpoints/archive",
    )
    .await;

    import(&client(), fixture.url, AUTH, &request, &store)
        .await
        .expect("valid import succeeds");

    let raw = fixture.request.await.expect("fixture completes");
    let header_end = raw
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .expect("request headers")
        + 4;
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
    assert!(headers.starts_with("put /proxy/runtime/checkpoints/archive http/1.1"));
    assert!(headers.contains(&format!("content-length: {}", bytes.len())));
    assert!(headers.contains("open-sandbox-api-key: provider-secret"));
    assert_eq!(&raw[header_end..], bytes);
}

#[tokio::test]
async fn import_rejects_store_identity_hash_and_runtime_status_mismatches() {
    let bytes = b"durable bytes";
    let request = restore(bytes);
    let invalid_store: Arc<dyn CheckpointArchiveStore> = Arc::new(DownloadStore {
        reference: "another-object".into(),
        sha256: request.sha256.clone(),
        size_bytes: bytes.len() as u64,
        chunks: Mutex::new(Some(vec![Ok(bytes.to_vec())])),
    });
    let error = import(
        &client(),
        Url::parse("http://127.0.0.1:1/checkpoint").expect("URL"),
        AUTH,
        &request,
        &invalid_store,
    )
    .await
    .expect_err("mismatched store metadata is refused before HTTP");
    assert!(matches!(error, AdapterError::Rejected(_)));

    let wrong = b"changed bytes";
    assert_eq!(wrong.len(), bytes.len());
    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(DownloadStore::new(
        &request,
        bytes.len() as u64,
        vec![Ok(wrong.to_vec())],
    ));
    let fixture = serve(
        response("204 No Content", &[("Content-Length", "0".into())], &[]),
        "/checkpoint",
    )
    .await;
    let error = import(&client(), fixture.url, AUTH, &request, &store)
        .await
        .expect_err("stream hash mismatch is refused");
    assert!(matches!(error, AdapterError::Rejected(_)));
    fixture.request.await.expect("fixture completes");

    let store: Arc<dyn CheckpointArchiveStore> = Arc::new(DownloadStore::new(
        &request,
        bytes.len() as u64,
        vec![Ok(bytes.to_vec())],
    ));
    let fixture = serve(
        response("200 OK", &[("Content-Length", "0".into())], &[]),
        "/checkpoint",
    )
    .await;
    let error = import(&client(), fixture.url, AUTH, &request, &store)
        .await
        .expect_err("runtime must acknowledge import with 204");
    assert!(matches!(error, AdapterError::Rejected(_)));
    fixture.request.await.expect("fixture completes");
}

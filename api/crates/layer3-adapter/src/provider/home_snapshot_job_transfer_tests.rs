use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use futures_util::SinkExt;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{
    handshake::server::{Callback, ErrorResponse, Request, Response},
    http::HeaderValue,
    Message,
};

use super::super::super::snapshot_views::tests::claim as view_claim;
use super::super::super::tests::row;
use super::super::tests::{pod, pod_row, ready, request};
use super::*;
use crate::hosted::{CheckpointArchiveDownload, CheckpointArchiveReceipt};
use crate::{CheckpointReceipt, WriterState};

enum Reply {
    Http((u16, Value)),
    Exec(Vec<u8>, Option<bool>),
}

struct Handshake(Arc<Mutex<Vec<String>>>);

impl Callback for Handshake {
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> std::result::Result<Response, ErrorResponse> {
        self.0.lock().unwrap().push(request.uri().to_string());
        response.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("v5.channel.k8s.io"),
        );
        Ok(response)
    }
}

struct Fixture {
    store: PersistentHomeVolumes,
    task: tokio::task::JoinHandle<()>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    async fn finish(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .unwrap()
            .unwrap();
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(replies: Vec<Reply>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            match reply {
                Reply::Http((code, value)) => {
                    let request = crate::provider::tests::read_request(&mut stream).await;
                    recorded.lock().unwrap().push(request);
                    let body = value.to_string();
                    let response = format!("HTTP/1.1 {code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                }
                Reply::Exec(bytes, outcome) => {
                    let mut socket =
                        tokio_tungstenite::accept_hdr_async(stream, Handshake(recorded.clone()))
                            .await
                            .unwrap();
                    for chunk in bytes.chunks(32 * 1024) {
                        let mut framed = vec![1];
                        framed.extend_from_slice(chunk);
                        if socket.send(Message::Binary(framed)).await.is_err() {
                            break;
                        }
                    }
                    if let Some(success) = outcome {
                        let status = json!({"apiVersion":"v1","kind":"Status","status":if success { "Success" } else { "Failure" }});
                        let mut framed = vec![3];
                        framed.extend_from_slice(status.to_string().as_bytes());
                        let _ = socket.send(Message::Binary(framed)).await;
                    }
                    let _ = socket.close(None).await;
                }
            }
        }
    });
    let client = kube::Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    Fixture {
        store: PersistentHomeVolumes::namespaced(client, "cma-test", "deployment").unwrap(),
        task,
        requests,
    }
}

#[derive(Default)]
struct Sink {
    committed: AtomicBool,
    bytes: Mutex<Vec<u8>>,
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for Sink {
    async fn upload(
        &self,
        mut archive: CheckpointArchiveUpload,
    ) -> Result<CheckpointArchiveReceipt> {
        let mut bytes = Vec::new();
        while let Some(chunk) = archive.chunks.recv().await {
            bytes.extend_from_slice(&chunk?);
        }
        assert_eq!(bytes.len() as u64, archive.size_bytes);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), archive.sha256);
        *self.bytes.lock().unwrap() = bytes;
        self.committed.store(true, Ordering::SeqCst);
        Ok(CheckpointArchiveReceipt {
            object_reference: archive.object_reference,
            sha256: archive.sha256,
            size_bytes: archive.size_bytes,
        })
    }

    async fn download(&self, _: &str) -> Result<CheckpointArchiveDownload> {
        Err(invalid())
    }
}

fn receipt(bytes: &[u8]) -> HomeSnapshotExportReceipt {
    let hash = format!("{:x}", Sha256::digest(bytes));
    HomeSnapshotExportReceipt {
        request_sha256: request().export.computed_sha256().unwrap(),
        checkpoint: CheckpointReceipt {
            epoch: 1,
            object_reference: format!("home-{hash}.cma"),
            sha256: hash,
            writers: WriterState::Resumed,
        },
        archive_bytes: bytes.len() as u64,
        conversations: vec![],
    }
}

fn observations() -> Vec<Reply> {
    vec![
        Reply::Http(pod_row(pod("export-one", Phase::Ready))),
        Reply::Http(row(view_claim("view-one", true))),
    ]
}

fn start(receipt: &HomeSnapshotExportReceipt) -> Vec<Reply> {
    let mut replies = observations();
    replies.push(Reply::Exec(
        serde_json::to_vec(receipt).unwrap(),
        Some(true),
    ));
    replies.extend(observations());
    replies
}

#[tokio::test]
async fn full_upload_checks_receipt_stream_remote_success_and_original_pod() {
    let bytes = vec![17; 180_000];
    let receipt = receipt(&bytes);
    let mut replies = start(&receipt);
    replies.push(Reply::Exec(bytes.clone(), Some(true)));
    replies.extend(observations());
    let fixture = serve(replies).await;
    let sink = Sink::default();
    let exported = fixture
        .store
        .upload_export(&request(), &ready(), &sink)
        .await
        .unwrap();
    assert_eq!(exported, receipt);
    assert!(sink.committed.load(Ordering::SeqCst));
    assert_eq!(*sink.bytes.lock().unwrap(), bytes);
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 8);
    for request in [&requests[2], &requests[5]] {
        let url = url::Url::parse(&format!("http://test{request}")).unwrap();
        let commands = url
            .query_pairs()
            .filter(|(key, _)| key == "command")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>();
        assert_eq!(commands[2], READ_FILE);
        assert_eq!(commands[4], "export-one");
        assert!(url
            .query_pairs()
            .any(|(key, value)| key == "container" && value == "transfer"));
        assert!(!url
            .query_pairs()
            .any(|(key, value)| key == "tty" && value == "true"));
    }
}

#[tokio::test]
async fn corrupt_truncated_oversized_or_unsuccessful_reads_never_commit() {
    let bytes = vec![17; 180_000];
    let receipt = receipt(&bytes);
    for mode in 0..6 {
        let mut body = bytes.clone();
        let status = match mode {
            3 => Some(false),
            4 => None,
            _ => Some(true),
        };
        match mode {
            0 => {
                body.pop();
            }
            1 => body[0] ^= 1,
            2 => body.push(0),
            _ => (),
        }
        let mut replies = start(&receipt);
        replies.push(Reply::Exec(body, status));
        if mode == 5 {
            replies.push(Reply::Http(pod_row(pod("replacement", Phase::Ready))));
        }
        let fixture = serve(replies).await;
        let sink = Sink::default();
        assert!(
            fixture
                .store
                .upload_export(&request(), &ready(), &sink)
                .await
                .is_err(),
            "mode {mode}"
        );
        assert!(!sink.committed.load(Ordering::SeqCst), "mode {mode}");
        assert_eq!(fixture.finish().await.len(), if mode == 5 { 7 } else { 6 });
    }
}

#[tokio::test]
async fn mismatched_or_oversized_receipt_cannot_start_an_archive_upload() {
    let mut invalid_receipt = receipt(b"fixture");
    invalid_receipt.request_sha256 = "0".repeat(64);
    for bytes in [
        serde_json::to_vec(&invalid_receipt).unwrap(),
        vec![b' '; MAX_RECEIPT_BYTES as usize + 1],
    ] {
        let mut replies = observations();
        replies.push(Reply::Exec(bytes, Some(true)));
        let fixture = serve(replies).await;
        let sink = Sink::default();
        assert!(fixture
            .store
            .upload_export(&request(), &ready(), &sink)
            .await
            .is_err());
        assert!(!sink.committed.load(Ordering::SeqCst));
        assert_eq!(fixture.finish().await.len(), 3);
    }
}

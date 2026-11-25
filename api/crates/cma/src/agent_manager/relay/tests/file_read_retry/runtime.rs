//! Synthetic provider API and receiving-runtime bytes; production ownership transport is unchanged.
use futures_util::{SinkExt, StreamExt};
use l3::hosted::{HostedControlTransport, NativeRuntimeTransport, ProviderHostedControlTransport};
use layer3_adapter as l3;
use serde_json::{json, Value};
use sha2::Digest;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{
    handshake::server::{Request, Response},
    Message,
};

pub(super) const HANDLE: &str = "agent-sandbox:claimed:sandbox-uid";

use super::{digest, FILE_BYTES, FILE_NAME, FILE_PATH};
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(super) enum Challenge {
    Accept,
    Refuse,
    Stall,
    LoseReply,
    Stale,
    ReplacePod,
}

#[derive(Default)]
pub(super) struct Counts {
    pub challenge: AtomicUsize,
    pub confirm: AtomicUsize,
    pub files: AtomicUsize,
    pub status: AtomicUsize,
    pub edits: AtomicUsize,
    pub edit_admissions: AtomicUsize,
    pub edited_bytes: Mutex<Option<Vec<u8>>>,
    pub lose_edit_reply: AtomicBool,
    pub challenges: Mutex<VecDeque<Challenge>>,
    pub lose_confirmation: AtomicBool,
    pub restarted: AtomicBool,
    pub entered: Notify,
    pub issued: Mutex<Vec<uuid::Uuid>>,
    pub confirmed: Mutex<Vec<uuid::Uuid>>,
}

pub(super) struct Runtime {
    pub transport: Arc<ProviderHostedControlTransport>,
    pub counts: Arc<Counts>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Runtime {
    pub(super) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = kube::Config::new(format!("http://{address}").parse().unwrap());
        config.default_retry = false;
        let client = l3::agent_sandbox::AgentSandboxClient::new(
            kube::Client::try_from(config).unwrap(),
            l3::agent_sandbox::AgentSandboxConfig {
                namespace: "fixture".into(),
                deployment: "fixture".into(),
                runtime_class: None,
                service_account: "default".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                template_fs_group_change_policy: None,
                native_claim_guard_enabled: false,
                runtime_startup_breaker_enabled: false,
                control_key: "fixture".repeat(8),
            },
            "fixture",
        )
        .unwrap();

        let transport = Arc::new(
            ProviderHostedControlTransport::for_agent_sandbox_with_transport(
                client,
                NativeRuntimeTransport::KubernetesExec,
            )
            .unwrap(),
        );
        transport
            .bind_native_runtime(
                &l3::RuntimeHandle::new(HANDLE),
                super::SANDBOX,
                &"a".repeat(64),
            )
            .await
            .unwrap();
        let counts = Arc::new(Counts::default());
        let observed = counts.clone();
        let server = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (socket, _) = connection.unwrap();
                        let counts = observed.clone();
                        tasks.spawn(async move { serve(socket, counts).await });
                    }
                    result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap(); }
                }
            }
        });
        Self {
            transport,
            counts,
            server,
        }
    }
}

async fn serve(mut socket: tokio::net::TcpStream, counts: Arc<Counts>) {
    let mut peek = vec![0; 64 * 1024];
    let header = loop {
        let size = socket.peek(&mut peek).await.unwrap();
        if size == 0 {
            return;
        }
        if peek[..size].windows(4).any(|w| w == b"\r\n\r\n") {
            break String::from_utf8(peek[..size].to_vec()).unwrap();
        }
        tokio::task::yield_now().await;
    };
    if !header.lines().next().unwrap().contains("/exec?") {
        let body = if header.contains("/sandboxes/claimed ") {
            json!({"apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox",
                "metadata":{"name":"claimed", "namespace":"fixture", "uid":"sandbox-uid",
                    "resourceVersion":"7", "generation":1, "labels":{"cma/control-plane":"fixture"}},
                "status":{"conditions":[{"type":"Ready", "status":"True", "observedGeneration":1}]}})
        } else {
            assert!(header.lines().next().unwrap().contains("/pods?"));
            let hash = "claimed".bytes().fold(2_166_136_261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16_777_619));
            let restarted = counts.restarted.load(Ordering::SeqCst);
            let image = "sha256:runtime";
            json!({"apiVersion":"v1", "kind":"PodList", "metadata":{"resourceVersion":"8"}, "items":[{
                "metadata":{"name":"claimed-pod", "namespace":"fixture", "uid":"original-pod",
                    "labels":{"cma/control-plane":"fixture", "agents.x-k8s.io/sandbox-name-hash":format!("{hash:08x}")},
                    "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox",
                        "name":"claimed", "uid":"sandbox-uid", "controller":true}]},
                "spec":{"containers":[{"name":"runtime"}]},
                "status":{"phase":"Running", "podIP":"10.0.0.1", "conditions":[{"type":"Ready", "status":"True"}],
                    "containerStatuses":[{"name":"runtime", "image":"runtime", "imageID":image,
                        "ready":true, "restartCount":usize::from(restarted),
                        "containerID":if restarted {"containerd://replacement"} else {"containerd://original"},
                        "state":{"running":{"startedAt":"2000-01-01T00:00:00Z"}}}]}}]})
        }.to_string();
        socket
            .write_all(response(200, &body).as_bytes())
            .await
            .unwrap();
        return;
    }
    #[allow(clippy::result_large_err)]
    let mut socket =
        tokio_tungstenite::accept_hdr_async(socket, |_: &Request, mut reply: Response| {
            reply.headers_mut().insert(
                "sec-websocket-protocol",
                "v5.channel.k8s.io".parse().unwrap(),
            );
            Ok(reply)
        })
        .await
        .unwrap();
    socket
        .send(Message::Binary(b"\x01CMA-EXEC-1\n".to_vec()))
        .await
        .unwrap();
    let mut bytes = Vec::new();
    let (head, body) = loop {
        match socket.next().await {
            Some(Ok(Message::Binary(chunk))) if chunk[0] == 0 => {
                bytes.extend_from_slice(&chunk[1..])
            }
            Some(Ok(Message::Ping(chunk))) => {
                socket.send(Message::Pong(chunk)).await.unwrap();
            }
            _ => return,
        }
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                break (head, bytes[end + 4..end + 4 + length].to_vec());
            }
        }
    };

    let path = head.lines().next().unwrap();
    let (code, reply) = if path.contains("/challenge ") {
        counts.challenge.fetch_add(1, Ordering::SeqCst);
        let behavior = counts
            .challenges
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Challenge::Accept);
        let nonce = uuid::Uuid::new_v4();
        let accepted = json!({"protocol":1, "nonce":nonce, "valid_for_ms":10000});
        if matches!(behavior, Challenge::Accept | Challenge::LoseReply) {
            counts.issued.lock().unwrap().push(nonce);
        }
        counts.entered.notify_one();
        match behavior {
            Challenge::Accept => (200, accepted),
            Challenge::Refuse => (403, json!({})),
            Challenge::Stale => (
                400,
                json!({"t":"failure", "code":"manager_ownership_stale", "retryable":false}),
            ),
            Challenge::ReplacePod => {
                counts.restarted.store(true, Ordering::SeqCst);
                (403, json!({}))
            }
            Challenge::Stall => {
                tokio::time::sleep(Duration::from_secs(4)).await;
                (200, accepted)
            }
            Challenge::LoseReply => {
                let mut partial = vec![1];
                partial.extend_from_slice(
                    response(200, &accepted.to_string())
                        .split_once("\r\n\r\n")
                        .unwrap()
                        .0
                        .as_bytes(),
                );
                partial.extend_from_slice(b"\r\n\r\n");
                let _ = socket.send(Message::Binary(partial)).await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                return;
            }
        }
    } else if path.contains("/confirm ") {
        counts.confirm.fetch_add(1, Ordering::SeqCst);
        let request: l3::owner_protocol::OwnerConfirmation = serde_json::from_slice(&body).unwrap();
        counts
            .confirmed
            .lock()
            .unwrap()
            .push(uuid::Uuid::parse_str(&request.nonce).unwrap());
        if counts.lose_confirmation.swap(false, Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(4)).await;
            return;
        }
        (200, json!({"protocol":1, "owner":request.owner}))
    } else if path.contains("/revoke ") {
        (
            200,
            json!({"protocol":1, "owner":serde_json::from_slice::<Value>(&body).unwrap()}),
        )
    } else {
        assert!(path.contains("/runtime/control "), "{path}");
        let frame: l3::wire::RequestFrame = serde_json::from_slice(&body).unwrap();
        let edit = matches!(frame, l3::wire::RequestFrame::WriteAgentFile(_));
        let result = control(frame, &counts).await;
        if edit && counts.lose_edit_reply.swap(false, Ordering::SeqCst) {
            return;
        }
        result
    };
    let mut response_bytes = vec![1];
    response_bytes.extend_from_slice(response(code, &reply.to_string()).as_bytes());
    if socket.send(Message::Binary(response_bytes)).await.is_ok() {
        let _ = socket
            .send(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec()))
            .await;
    }
}

async fn control(frame: l3::wire::RequestFrame, counts: &Counts) -> (u16, Value) {
    use base64::Engine as _;
    use l3::wire::{RequestFrame, ResponseFrame};
    let frame =
        match frame {
            RequestFrame::Resources => ResponseFrame::Resources(l3::ResourceReading::default()),
            RequestFrame::RuntimeStatus(request) => {
                counts.status.fetch_add(1, Ordering::SeqCst);
                ResponseFrame::RuntimeStatus(serde_json::from_value(json!({
                "sandbox_id": request.sandbox_id, "generation": request.generation,
                "home_identity": request.home_identity, "boot_id": "a".repeat(32),
                "installation_ready": true, "credential_ready": true, "restore_pending": false,
                "agent_filesystem": true, "agent_processes": 1
            })).unwrap())
            }
            RequestFrame::ListAgentFiles(request) => {
                counts.files.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.path, "cma");
                assert!(request.owner_access);
                ResponseFrame::ListAgentFiles(l3::AgentFileListReceipt {
                    path: request.path,
                    entries: vec![l3::AgentFileEntry {
                        path: FILE_PATH.into(),
                        name: FILE_NAME.into(),
                        kind: l3::AgentFileKind::File,
                        size_bytes: FILE_BYTES.len() as u64,
                        sha256: Some(digest()),
                        hidden: false,
                    }],
                })
            }
            RequestFrame::ReadAgentFile(request) => {
                counts.files.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.path, FILE_PATH);
                assert!(request.owner_access);
                ResponseFrame::ReadAgentFile(l3::AgentFileReadReceipt {
                    path: request.path,
                    size_bytes: FILE_BYTES.len() as u64,
                    sha256: digest(),
                    content_base64: base64::engine::general_purpose::STANDARD.encode(FILE_BYTES),
                })
            }
            RequestFrame::WriteAgentFile(request) => {
                assert_eq!(request.path, FILE_PATH);
                assert!(request.owner_access);
                assert!(!request.directory);
                counts.edits.fetch_add(1, Ordering::SeqCst);
                let mut stored = counts.edited_bytes.lock().unwrap();
                let original = stored.as_deref().unwrap_or(FILE_BYTES);
                let actual = format!("{:x}", sha2::Sha256::digest(original));
                if request.expected_sha256.as_ref() != Some(&actual) {
                    return (
                        409,
                        json!({"t":"failure", "code":"file_conflict", "retryable":false}),
                    );
                }
                let bytes = request.content().unwrap();
                let receipt = l3::AgentFileWriteReceipt {
                    path: request.path,
                    size_bytes: bytes.len() as u64,
                    sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
                };
                *stored = Some(bytes);
                ResponseFrame::WriteAgentFile(receipt)
            }
            other => panic!("unexpected fixture operation {other:?}"),
        };
    (200, serde_json::to_value(frame).unwrap())
}

fn response(status: u16, body: &str) -> String {
    format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len())
}

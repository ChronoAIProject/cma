//! Synthetic provider API and receiving-runtime bytes; production ownership transport is unchanged.
use futures_util::{SinkExt, StreamExt};
use l3::hosted::{
    HostedBackend, HostedControlTransport, InMemoryHostedBackend, LegacyReceiverImage,
    NativeRuntimeTransport, ProviderHostedControlTransport,
};
use layer3_adapter as l3;
use serde_json::{json, Value};
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

pub(super) struct Backend(
    pub(super) Arc<ProviderHostedControlTransport>,
    pub(super) InMemoryHostedBackend,
);

#[async_trait::async_trait]
impl HostedBackend for Backend {
    async fn adopt(
        &self,
        handle: &l3::RuntimeHandle,
        _: &l3::provider::ProviderSandboxRequest,
    ) -> l3::Result<Option<l3::RuntimeObservation>> {
        self.0
            .exchange(handle, l3::wire::RequestFrame::Resources)
            .await?;
        Ok(None)
    }
    async fn provision(
        &self,
        request: l3::provider::ProviderSandboxRequest,
    ) -> l3::Result<l3::RuntimeHandle> {
        self.1.provision(request).await
    }
    async fn exchange(
        &self,
        handle: &l3::RuntimeHandle,
        request: l3::wire::RequestFrame,
    ) -> l3::Result<l3::wire::ResponseFrame> {
        self.0.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &l3::RuntimeHandle,
        launch: l3::wire::FencedRequest<l3::AgentLaunch>,
        resume: Option<String>,
        settings: l3::hosted::HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<l3::ConversationUpdate>,
    ) -> l3::Result<(
        l3::ConversationChannel,
        l3::wire::FencedReceipt<l3::wire::AgentOpenReceipt>,
    )> {
        self.1
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &l3::RuntimeHandle) -> l3::Result<()> {
        self.1.teardown(handle).await
    }
}

#[derive(Default)]
pub(super) struct Counts {
    challenge: AtomicUsize,
    confirm: AtomicUsize,
    control: AtomicUsize,
    acp: AtomicUsize,
    revoke: AtomicUsize,
    pub(super) cleanup: AtomicBool,
    restarted: AtomicBool,
    pub(super) replace_after_revocation: AtomicBool,
}

pub(super) struct Runtime {
    pub(super) transport: Arc<ProviderHostedControlTransport>,
    pub(super) counts: Arc<Counts>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Runtime {
    pub(super) async fn start(failure: &'static str) -> Self {
        Self::start_with_legacy_compatibility(failure, false).await
    }

    pub(super) async fn start_with_legacy_compatibility(
        failure: &'static str,
        enabled: bool,
    ) -> Self {
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
        let legacy_image = enabled.then(|| {
            LegacyReceiverImage::from_reviewed_mapping(
                format!("ghcr.io/fixture/runtime@sha256:{}", "1".repeat(64)),
                legacy_config_digest(),
                "bc8cd8bf0f585446a37d3bd50a8e3df2d6d06e6e",
            )
            .unwrap()
        });
        let transport = Arc::new(
            ProviderHostedControlTransport::for_agent_sandbox_with_transport(
                client,
                NativeRuntimeTransport::KubernetesExec,
            )
            .unwrap()
            .with_legacy_receiver_image(legacy_image),
        );
        let counts = Arc::new(Counts::default());
        let observed = counts.clone();
        let server = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (socket, _) = connection.unwrap();
                        let counts = observed.clone();
                        tasks.spawn(async move { serve(socket, counts, failure).await });
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
    pub(super) fn assert_counts(&self, probes: usize) {
        self.assert_counts_with_confirmation(probes, 0);
    }
    pub(super) fn revocations(&self) -> usize {
        self.counts.revoke.load(Ordering::SeqCst)
    }
    pub(super) fn assert_counts_with_confirmation(&self, probes: usize, confirmations: usize) {
        assert!(!self.server.is_finished(), "runtime fixture failed");
        assert_eq!(self.counts.challenge.load(Ordering::SeqCst), probes);
        assert_eq!(self.counts.confirm.load(Ordering::SeqCst), confirmations);
        assert_eq!(self.counts.control.load(Ordering::SeqCst), 0);
        assert_eq!(self.counts.acp.load(Ordering::SeqCst), 0);
    }
}

fn legacy_config_digest() -> String {
    format!("sha256:{}", "2".repeat(64))
}

async fn serve(mut socket: tokio::net::TcpStream, counts: Arc<Counts>, failure: &str) {
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
            let image = match failure {
                "receiverless" | "receiverless_confirm_ambiguous" => legacy_config_digest(),
                _ => "sha256:runtime".into(),
            };
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
        match failure {
            "capability_missing" | "receiverless" | "receiverless_unknown_image" => {
                (404, json!({}))
            }
            "challenge_protocol_mismatch" => (
                200,
                json!({"protocol":2, "nonce":uuid::Uuid::new_v4(), "valid_for_ms":10000}),
            ),
            "challenge_lifetime_mismatch" => (
                200,
                json!({"protocol":1, "nonce":uuid::Uuid::new_v4(), "valid_for_ms":9999}),
            ),
            "malformed" => (
                200,
                json!({"protocol":2, "nonce":"malformed", "valid_for_ms":10000}),
            ),
            "auth" => (403, json!({})),
            "confirm_ambiguous" | "receiverless_confirm_ambiguous" => (
                200,
                json!({"protocol":1, "nonce":uuid::Uuid::new_v4(), "valid_for_ms":10000}),
            ),
            "timeout" => {
                std::future::pending::<()>().await;
                unreachable!()
            }
            _ => panic!("unknown fixture failure"),
        }
    } else if path.contains("/revoke ") {
        counts.revoke.fetch_add(1, Ordering::SeqCst);
        if counts.replace_after_revocation.load(Ordering::SeqCst) {
            counts.restarted.store(true, Ordering::SeqCst);
        }
        (
            if failure.starts_with("receiverless") {
                404
            } else if counts.cleanup.load(Ordering::SeqCst) {
                200
            } else {
                403
            },
            json!({"protocol":1, "owner":serde_json::from_slice::<Value>(&body).unwrap()}),
        )
    } else {
        if path.contains("/confirm ") {
            counts.confirm.fetch_add(1, Ordering::SeqCst);
            return;
        } else if path.contains("/runtime/control ") {
            counts.control.fetch_add(1, Ordering::SeqCst);
        } else {
            counts.acp.fetch_add(1, Ordering::SeqCst);
        }
        (500, json!({}))
    };
    let mut response_bytes = vec![1];
    response_bytes.extend_from_slice(response(code, &reply.to_string()).as_bytes());
    // The HTTP body is drained even when exec exit is already buffered.
    if socket.send(Message::Binary(response_bytes)).await.is_ok() {
        let _ = socket
            .send(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec()))
            .await;
    }
}

fn response(status: u16, body: &str) -> String {
    format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len())
}

pub(super) struct Home(pub(super) l3::HomeVolumeAttachment);
#[async_trait::async_trait]
impl l3::HomeVolumeStore for Home {
    async fn inventory(&self) -> l3::Result<Vec<l3::HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }
    async fn prepare(&self, _: &l3::HomeVolumeRequest) -> l3::Result<l3::HomeVolumeReceipt> {
        panic!("retained home is already allocated")
    }
    async fn verify(&self, home: &l3::HomeVolumeAttachment) -> l3::Result<()> {
        assert_eq!(home, &self.0);
        Ok(())
    }
    async fn remove(
        &self,
        _: &l3::HomeVolumeRequest,
        _: Option<&l3::HomeVolumeReceipt>,
    ) -> l3::Result<()> {
        panic!("reconnect cannot delete its retained home")
    }
}

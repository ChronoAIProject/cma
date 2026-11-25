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
        request: &l3::provider::ProviderSandboxRequest,
    ) -> l3::Result<Option<l3::RuntimeObservation>> {
        self.0
            .exchange(handle, l3::wire::RequestFrame::Resources)
            .await?;
        let meta = &request.metadata;
        Ok(Some(l3::RuntimeObservation {
            handle: handle.clone(),
            sandbox_id: meta["cma/sandbox"].clone(),
            generation: meta["cma/runtime-generation"].parse().unwrap(),
            image_sha256: meta["cma/image-sha256"].clone(),
            spec_fingerprint: meta["cma/spec-sha256"].clone(),
            runtime_contract: meta["cma/runtime-contract"].clone(),
            lifecycle: l3::RuntimeLifecycle {
                state: "running".into(),
                reason: None,
            },
        }))
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
        let Some(conversation) = resume else {
            return self
                .1
                .open_agent(handle, launch, None, settings, updates)
                .await;
        };
        let request = launch.request;
        // Keep the retained output stream alive until the worker detaches.
        tokio::spawn(async move { updates.closed().await });
        Ok((
            l3::ConversationChannel {
                input: l3::ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                conversation_id: Some(conversation.clone()),
            },
            l3::wire::FencedReceipt::new(
                launch.authority_fence,
                l3::wire::AgentOpenReceipt {
                    operation_id: request.operation_id,
                    request_sha256: request.request_sha256,
                    agent_id: request.agent_id,
                    cwd: request.cwd,
                    home: request.home,
                    conversation_id: Some(conversation),
                },
            ),
        ))
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
    repaired: AtomicBool,
    hold_first_close: AtomicBool,
    close_received: tokio::sync::Notify,
    close_reply: tokio::sync::Notify,
    close_receipt: std::sync::Mutex<Option<l3::wire::AgentCloseReceipt>>,
    closes: AtomicUsize,
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
    pub(super) fn repair(&self) {
        self.counts.repaired.store(true, Ordering::SeqCst);
    }

    pub(super) fn hold_first_close(&self) {
        self.counts.hold_first_close.store(true, Ordering::SeqCst);
    }

    pub(super) async fn close_received(&self) {
        self.counts.close_received.notified().await;
    }

    pub(super) fn release_close(&self) {
        self.counts.close_reply.notify_one();
    }

    pub(super) fn assert_close_replayed(&self) {
        assert_eq!(self.counts.closes.load(Ordering::SeqCst), 2);
        assert!(self.counts.close_receipt.lock().unwrap().is_some());
    }

    pub(super) fn assert_repaired_probe(&self, controls: usize) {
        assert_eq!(self.counts.challenge.load(Ordering::SeqCst), controls + 1);
        assert_eq!(self.counts.confirm.load(Ordering::SeqCst), controls);
        assert_eq!(self.counts.control.load(Ordering::SeqCst), controls);
        assert_eq!(self.counts.acp.load(Ordering::SeqCst), 0);
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
            "auth" if counts.repaired.load(Ordering::SeqCst) => (
                200,
                json!({"protocol":1, "nonce":uuid::Uuid::new_v4(), "valid_for_ms":10000}),
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
        let repaired = counts.repaired.load(Ordering::SeqCst);
        if path.contains("/confirm ") {
            counts.confirm.fetch_add(1, Ordering::SeqCst);
            if !repaired {
                return;
            }
            let request = serde_json::from_slice::<Value>(&body).unwrap();
            (200, json!({"protocol": 1, "owner": request["owner"]}))
        } else if path.contains("/runtime/control ") {
            counts.control.fetch_add(1, Ordering::SeqCst);
            if repaired {
                (200, json!(repair_response(&body, &counts).await))
            } else {
                (500, json!({}))
            }
        } else {
            counts.acp.fetch_add(1, Ordering::SeqCst);
            (500, json!({}))
        }
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

async fn repair_response(bytes: &[u8], counts: &Counts) -> l3::wire::ResponseFrame {
    use l3::wire::{AgentCloseReceipt, FencedReceipt, RequestFrame, ResponseFrame};
    match serde_json::from_slice::<RequestFrame>(bytes).unwrap() {
        RequestFrame::Resources => ResponseFrame::Resources(Default::default()),
        RequestFrame::RuntimeStatus(request) => ResponseFrame::RuntimeStatus(l3::RuntimeStatus {
            agent_browser: false,
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: false,
            agent_skill_upgrade: false,
            agent_model_catalog: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            sandbox_id: request.sandbox_id,
            generation: request.generation,
            home_identity: request.home_identity,
            boot_id: "a".repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_processes: 1,
        }),
        RequestFrame::CloseAgent(close) => {
            let receipt = AgentCloseReceipt {
                operation_id: close.request.operation_id,
                request_sha256: close.request.request_sha256,
                agent_id: close.request.agent_id,
            };
            {
                let mut durable = counts.close_receipt.lock().unwrap();
                if let Some(previous) = &*durable {
                    assert_eq!(previous, &receipt);
                } else {
                    *durable = Some(receipt.clone());
                }
            }
            if counts.closes.fetch_add(1, Ordering::SeqCst) == 0
                && counts.hold_first_close.load(Ordering::SeqCst)
            {
                counts.close_received.notify_one();
                counts.close_reply.notified().await;
            }
            ResponseFrame::CloseAgent(FencedReceipt::new(close.authority_fence, receipt))
        }
        other => panic!(
            "unexpected repaired runtime request: {:?}",
            other.operation()
        ),
    }
}

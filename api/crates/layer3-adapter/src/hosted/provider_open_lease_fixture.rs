//! Loopback receiver/Open/ACP fixture; no external service or durable store.
use super::*;
use crate::owner_protocol::{
    scope, test_support::TestAuthority, ReceiverRevocation, TransportAuthority, TransportOwner,
    OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse};

struct OwnedHandshake(String);

impl Callback for OwnedHandshake {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        assert_eq!(
            request.uri().path(),
            "/proxy/runtime/agents/agent-1/conversation"
        );
        assert_eq!(
            request.headers()[crate::owner_protocol::OWNER_HEADER],
            self.0
        );
        Ok(response)
    }
}

pub(super) struct CountingAuthority {
    pub(super) inner: Arc<TestAuthority>,
    delay: Duration,
    pub(super) registrations: AtomicUsize,
    pub(super) receivers: StdMutex<Vec<String>>,
}

#[async_trait::async_trait]
impl TransportAuthority for CountingAuthority {
    fn owner(&self) -> TransportOwner {
        self.inner.owner()
    }
    fn is_current(&self) -> bool {
        self.inner.is_current()
    }
    fn register_receiver(&self, receiver: Arc<dyn ReceiverRevocation>) -> crate::Result<()> {
        self.registrations.fetch_add(1, Ordering::SeqCst);
        self.receivers.lock().unwrap().push(receiver.identity());
        self.inner.register_receiver(receiver)
    }
    async fn check(&self) -> crate::Result<()> {
        self.inner.check().await
    }
    async fn renew_for_challenge(&self) -> crate::Result<()> {
        tokio::time::sleep(self.delay).await;
        self.inner.renew_for_challenge().await
    }
    async fn revoked(&self) {
        self.inner.revoked().await
    }
    fn revoke(&self) {
        self.inner.revoke();
    }
}

#[derive(Default)]
pub(super) struct Counts {
    pub(super) challenge: AtomicUsize,
    pub(super) confirm: AtomicUsize,
    pub(super) open: AtomicUsize,
    pub(super) websocket: AtomicUsize,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Failure {
    None,
    Renewal,
    Ack,
    Receipt,
    Handshake,
    RevokeAfterOpen,
    RevokeDuringHandshake,
}

pub(super) struct Fixture {
    pub(super) transport: ProviderHostedControlTransport,
    pub(super) authority: Arc<CountingAuthority>,
    pub(super) counts: Arc<Counts>,
    provider: tokio::task::JoinHandle<()>,
    pub(super) discoveries: Arc<AtomicUsize>,
    runtime: tokio::task::JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    _received: tokio::sync::mpsc::Receiver<ConversationUpdate>,
}

impl Fixture {
    pub(super) async fn new(epoch: u64, delay_ms: u64, failure: Failure) -> Self {
        let runtime = listener().await;
        let address = runtime.local_addr().unwrap();
        let authority = Arc::new(CountingAuthority {
            inner: TestAuthority::new(epoch),
            delay: Duration::from_millis(delay_ms / 2),
            registrations: AtomicUsize::new(0),
            receivers: StdMutex::default(),
        });
        authority
            .inner
            .fail_renewal
            .store(failure == Failure::Renewal, Ordering::SeqCst);
        let counts = Arc::new(Counts::default());
        let counted = counts.clone();
        let owner = authority.owner();
        let observed_authority = authority.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let runtime = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            let mut challenge = None;
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stopped => break,
                    accepted = runtime.accept() => accepted.unwrap(),
                };
                let mut peek = [0; 4];
                let n = stream.peek(&mut peek).await.unwrap();
                if n >= 3 && &peek[..3] == b"GET" {
                    counted.websocket.fetch_add(1, Ordering::SeqCst);
                    let websocket_owner = owner.clone();
                    let websocket_authority = observed_authority.clone();
                    children.spawn(async move {
                        let mut socket = tokio_tungstenite::accept_hdr_async(
                            stream, OwnedHandshake(websocket_owner.header().unwrap()),
                        ).await.unwrap();
                        socket.send(Message::Text(serde_json::json!({
                            "t":"ready","wireVersion":1,"lanes":["ctl","conv"],"agentId":"agent-1",
                            "managerOwnership":{"protocol":1,"owner":websocket_owner},
                            "outputAcknowledgement":{"version":1,"streamId":uuid::Uuid::from_u128(1)}
                        }).to_string())).await.unwrap();
                        if failure == Failure::RevokeDuringHandshake {
                            websocket_authority.revoke();
                        }
                        if matches!(failure, Failure::Handshake | Failure::RevokeDuringHandshake) {
                            while socket.next().await.is_some() {}
                            return;
                        }
                        for (cursor, (method, result)) in [("initialize", serde_json::json!({})),
                            ("session/new", serde_json::json!({"sessionId":"fixture-conversation"}))].into_iter().enumerate() {
                            let request = loop {
                                let next = socket.next().await.unwrap().unwrap();
                                if let Message::Binary(bytes) = next {
                                    if bytes.first() == Some(&crate::hosted::acp::LANE_CONVERSATION) {
                                        break serde_json::from_slice::<serde_json::Value>(&bytes[1..]).unwrap();
                                    }
                                }
                            };
                            assert_eq!(request["method"], method);
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            let mut frame = vec![0x04];
                            frame.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
                            frame.extend_from_slice(&((cursor + 1) as u64).to_be_bytes());
                            frame.extend(serde_json::to_vec(&serde_json::json!({
                                "jsonrpc":"2.0","id":request["id"],"result":result
                            })).unwrap());
                            socket.send(Message::Binary(frame)).await.unwrap();
                        }
                        while let Some(frame) = socket.next().await {
                            if matches!(frame, Ok(Message::Close(_)) | Err(_)) { break; }
                        }
                    });
                    continue;
                }
                let request = request_text(read_request(&mut stream).await);
                let response = if request.contains("/manager-ownership/challenge") {
                    counted.challenge.fetch_add(1, Ordering::SeqCst);
                    let requested: TransportOwner =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let nonce = uuid::Uuid::new_v4().to_string();
                    challenge = Some((requested, nonce.clone()));
                    serde_json::json!({"protocol":OWNER_PROTOCOL_VERSION,
                        "nonce":nonce,"valid_for_ms":RECEIVER_LEASE_MS})
                } else if request.contains("/manager-ownership/confirm") {
                    counted.confirm.fetch_add(1, Ordering::SeqCst);
                    let confirmation: crate::owner_protocol::OwnerConfirmation =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let (challenged_owner, nonce) = challenge.take().expect("fresh challenge");
                    assert_eq!(confirmation.nonce, nonce);
                    assert_eq!(confirmation.owner, challenged_owner);
                    if confirmation.owner == owner {
                        assert!(
                            observed_authority.inner.renewals.load(Ordering::SeqCst)
                                >= counted.confirm.load(Ordering::SeqCst)
                        );
                    }
                    let mut ack_owner = owner.clone();
                    if failure == Failure::Ack {
                        ack_owner.epoch += 1;
                    }
                    serde_json::json!({"protocol":OWNER_PROTOCOL_VERSION,"owner":ack_owner})
                } else {
                    assert!(request.starts_with("POST /proxy/runtime/control "));
                    assert!(request.contains(&owner.header().unwrap()));
                    assert!(counted.confirm.load(Ordering::SeqCst) > 0);
                    counted.open.fetch_add(1, Ordering::SeqCst);
                    let frame: RequestFrame =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let RequestFrame::OpenAgent(launch) = frame else {
                        panic!("only Open expected")
                    };
                    let response = ResponseFrame::OpenAgent(FencedReceipt::new(
                        if failure == Failure::Receipt {
                            999
                        } else {
                            launch.authority_fence
                        },
                        AgentOpenReceipt {
                            operation_id: launch.request.operation_id,
                            request_sha256: launch.request.request_sha256,
                            agent_id: launch.request.agent_id,
                            cwd: launch.request.cwd,
                            home: launch.request.home,
                            conversation_id: None,
                        },
                    ));
                    if failure == Failure::RevokeAfterOpen {
                        observed_authority.revoke();
                    }
                    serde_json::to_value(response).unwrap()
                };
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                if let Err(error) = stream.write_all(&json_response(&response)).await {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
                if let Err(error) = stream.shutdown().await {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::NotConnected
                    ));
                }
            }
            children.abort_all();
            while let Some(result) = children.join_next().await {
                if let Err(error) = result {
                    assert!(error.is_cancelled(), "{error}");
                }
            }
        });
        let provider = listener().await;
        let endpoint = serde_json::json!({"endpoint":format!("{address}/proxy")});
        let transport = ProviderHostedControlTransport::new(
            SandboxClient::new(
                format!("http://{}", provider.local_addr().unwrap()),
                "synthetic-key",
            ),
            Arc::new(RecordingFactory::default()),
        )
        .unwrap();
        let discoveries = Arc::new(AtomicUsize::new(0));
        let discovered = discoveries.clone();
        let provider = tokio::spawn(async move {
            loop {
                let (mut stream, _) = provider.accept().await.unwrap();
                read_request(&mut stream).await;
                discovered.fetch_add(1, Ordering::SeqCst);
                stream.write_all(&json_response(&endpoint)).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        let (updates, _received) = tokio::sync::mpsc::channel(8);
        Self {
            transport,
            authority,
            counts,
            provider,
            discoveries,
            runtime,
            stop,
            updates,
            _received,
        }
    }

    pub(super) async fn open(
        &self,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.open_as(self.authority.clone()).await
    }

    pub(super) async fn open_as(
        &self,
        authority: Arc<dyn TransportAuthority>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let launch = FencedRequest::new(
            9,
            AgentLaunch {
                operation_id: "agent:agent-1:open:v1".into(),
                request_sha256: "a".repeat(64),
                agent_id: AgentSlot::new("agent-1"),
                cwd: "/workspace".into(),
                home: "/agent-home".into(),
                program: "codex-acp".into(),
                args: vec![],
                env: BTreeMap::new(),
            },
        );
        scope(
            authority,
            self.transport.open_agent(
                &RuntimeHandle::new("runtime-1"),
                launch,
                None,
                HostedAgentOpenSettings {
                    coordinated_ingress: false,
                    retained_json: false,
                    bounded_raw_output: false,
                    bounded_permission_state: false,
                    output_json_budget: false,
                    shared_runtime_output: false,
                    already_initialized_error: None,
                    open_completion: None,
                    context_refresh: None,
                    permission_mode: None,
                    own_credential: false,
                    channel_tool: None,
                    provider_tool: None,
                    access_tool: None,
                    recovered_prompt: None,
                    recovered_context_sequence: None,
                },
                self.updates.clone(),
            ),
        )
        .await
    }

    pub(super) async fn finish(self) {
        self.transport
            .retire(&RuntimeHandle::new("runtime-1"))
            .await
            .unwrap();
        self.stop.send(()).expect("runtime fixture is live");
        self.runtime.await.unwrap();
        self.provider.abort();
        assert!(self.provider.await.unwrap_err().is_cancelled());
        assert!(self.transport.state.lock().await.links.is_empty());
    }
}

use super::*;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    routing::{get, post},
    Json, Router,
};
use layer3_adapter::hosted::{
    HostedAgentOpenSettings, HostedBackend, HostedControlTransport, ProviderHostedControlTransport,
};
use layer3_adapter::provider::{ProviderSandboxRequest, SandboxClient};
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{AgentLaunch, ConversationChannel, PromptIdentity, RuntimeObservation};
use serde_json::{json, Value};
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(super) enum Policy {
    Legacy,
    Shared,
    Coordinated,
}

pub(super) const STREAM: &str = "d99003d1-0c3c-46cd-aa47-9bbdbed50b12";

pub(super) struct Journal {
    pub agent: Agent,
    pub chunks: u64,
    pub acked: Mutex<Vec<u64>>,
    pub requests: Mutex<Vec<String>>,
    pub fail_settings: std::sync::atomic::AtomicBool,
    pub initialized: Notify,
    pub release: Notify,
    pub hold_handshake: std::sync::atomic::AtomicBool,
    pub closed: Notify,
}

pub(super) struct Backend {
    pub memory: InMemoryHostedBackend,
    pub transport: ProviderHostedControlTransport,
}

#[async_trait::async_trait]
impl HostedBackend for Backend {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.memory.provision(request).await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.memory.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        self.memory.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        assert!(resume.is_some());
        assert!(settings.recovered_prompt.is_some());
        self.transport
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.transport.retire(handle).await?;
        self.memory.teardown(handle).await
    }
}

pub(super) async fn start(
    h: &Harness,
    agent: Agent,
    chunks: u64,
    bounded: bool,
    policy: Policy,
    hold_handshake: bool,
) -> (Arc<Backend>, Arc<Journal>, tokio::task::JoinHandle<()>) {
    let journal = Arc::new(Journal {
        agent,
        chunks,
        acked: Mutex::new(vec![]),
        requests: Mutex::new(vec![]),
        fail_settings: std::sync::atomic::AtomicBool::new(false),
        initialized: Notify::new(),
        release: Notify::new(),
        hold_handshake: std::sync::atomic::AtomicBool::new(hold_handshake),
        closed: Notify::new(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = origin.clone();
    let router = Router::new()
        .route(
            "/v1/sandboxes/{runtime}/endpoints/{port}",
            get(move || {
                let endpoint = endpoint.clone();
                async move { Json(json!({"endpoint": endpoint})) }
            }),
        )
        .route(
            "/runtime/control",
            post(|Json(request): Json<RequestFrame>| async move {
                let RequestFrame::OpenAgent(request) = request else {
                    panic!("only original open is allowed")
                };
                let launch = request.request;
                Json(ResponseFrame::OpenAgent(FencedReceipt::new(
                    request.authority_fence,
                    AgentOpenReceipt {
                        operation_id: launch.operation_id,
                        request_sha256: launch.request_sha256,
                        agent_id: launch.agent_id,
                        cwd: launch.cwd,
                        home: launch.home,
                        conversation_id: None,
                    },
                )))
            }),
        )
        .route(
            "/runtime/agents/{agent}/conversation",
            get(
                |Path(agent): Path<String>,
                 State(journal): State<Arc<Journal>>,
                 ws: WebSocketUpgrade| async move {
                    assert_eq!(agent, journal.agent.id.as_str());
                    ws.on_upgrade(move |socket| serve(socket, journal))
                },
            ),
        )
        .with_state(journal.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        origin,
        "synthetic-provider",
    ))
    .unwrap()
    .with_bounded_update_relay(bounded)
    .with_shared_runtime_output(!matches!(policy, Policy::Legacy))
    .with_retained_json(matches!(policy, Policy::Coordinated))
    .with_bounded_raw_output(matches!(policy, Policy::Coordinated))
    .with_bounded_permission_state(matches!(policy, Policy::Coordinated))
    .with_output_json_budget(matches!(policy, Policy::Coordinated))
    .with_coordinated_ingress(matches!(policy, Policy::Coordinated));
    (
        Arc::new(Backend {
            memory: h.backend.clone(),
            transport,
        }),
        journal,
        server,
    )
}

async fn retained(socket: &mut WebSocket, cursor: u64, value: Value) -> bool {
    let mut frame = vec![4];
    frame.extend_from_slice(uuid::Uuid::parse_str(STREAM).unwrap().as_bytes());
    frame.extend_from_slice(&cursor.to_be_bytes());
    frame.extend(serde_json::to_vec(&value).unwrap());
    socket.send(Message::Binary(frame.into())).await.is_ok()
}

struct Closed(Arc<Journal>);
impl Drop for Closed {
    fn drop(&mut self) {
        self.0.closed.notify_one();
    }
}

async fn serve(mut socket: WebSocket, journal: Arc<Journal>) {
    let _closed = Closed(journal.clone());
    let agent = &journal.agent;
    socket
        .send(Message::Text(
            json!({"t":"ready","wireVersion":1,"lanes":["ctl","conv"],"agentId":agent.id,
        "outputAcknowledgement":{"version":1,"streamId":STREAM}})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let Some(Ok(Message::Binary(frame))) = socket.recv().await else {
        return;
    };
    assert_eq!(frame[0], 2);
    let initialize: Value = serde_json::from_slice(&frame[1..]).unwrap();
    assert_eq!(initialize["method"], "initialize");
    let conversation = agent.conversation.conversation_id.as_ref().unwrap();
    for cursor in 1..=journal.chunks {
        if !retained(&mut socket, cursor, json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":conversation,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}
        }})).await { return; }
    }
    let prompt = PromptIdentity::new(
        conversation,
        agent.conversation.runtime_generation.unwrap(),
        agent.conversation.active_turn.unwrap(),
    );
    if !retained(
        &mut socket,
        journal.chunks + 1,
        json!({"jsonrpc":"2.0","id":prompt.as_str(),"result":{"stopReason":"end_turn"}}),
    )
    .await
    {
        return;
    }
    journal.initialized.notify_one();
    if journal.hold_handshake.swap(false, Ordering::SeqCst) {
        tokio::select! {
            _ = journal.release.notified() => {},
            frame = socket.recv() => {
                assert!(!matches!(frame, Some(Ok(Message::Binary(_)))), "nothing may ACK before handshake");
                return;
            }
        }
    }
    if !retained(
        &mut socket,
        journal.chunks + 2,
        json!({"jsonrpc":"2.0","id":initialize["id"],"error":{
            "code":-32603,"message":"Internal error","data":{"details":"Already initialized"}
        }}),
    )
    .await
    {
        return;
    }
    let mut cursor = journal.chunks + 2;
    let mut refused_settings = None;
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Binary(frame) if frame[0] == 2 => {
                let request: Value = serde_json::from_slice(&frame[1..]).unwrap();
                let method = request["method"].as_str().unwrap();
                assert!(
                    matches!(
                        method,
                        "session/set_model" | "session/set_config_option" | "session/set_mode"
                    ),
                    "unexpected replay: {method}"
                );
                journal.requests.lock().unwrap().push(method.to_owned());
                if method == "session/set_model"
                    && journal.fail_settings.swap(false, Ordering::SeqCst)
                {
                    refused_settings = Some(request["id"].clone());
                    continue;
                }
                let result = if method == "session/set_config_option" {
                    json!({"configOptions":[{"id":"reasoning_effort","currentValue":request["params"]["value"]}]})
                } else {
                    json!({})
                };
                cursor += 1;
                if !retained(
                    &mut socket,
                    cursor,
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                )
                .await
                {
                    return;
                }
            }
            Message::Binary(frame) => {
                assert_eq!(
                    frame[0], 5,
                    "reconnect must not replay the prompt or load another session"
                );
                assert_eq!(
                    &frame[1..17],
                    uuid::Uuid::parse_str(STREAM).unwrap().as_bytes()
                );
                journal
                    .acked
                    .lock()
                    .unwrap()
                    .push(u64::from_be_bytes(frame[17..25].try_into().unwrap()));
            }
            Message::Close(_) => break,
            _ => {}
        }
        if journal
            .acked
            .lock()
            .unwrap()
            .last()
            .is_some_and(|at| *at > journal.chunks)
        {
            if let Some(id) = refused_settings.take() {
                cursor += 1;
                if !retained(&mut socket, cursor, json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"synthetic settings refusal"}})).await { return; }
            }
        }
    }
}

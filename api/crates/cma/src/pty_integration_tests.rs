//! Public PTY WebSocket contract tests at the composed control-plane boundary.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    AgentConfiguration, AgentOptions, AttachmentClaim, Authorization, Baseline, Caller, Capability,
    Deployment, EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
    EnvironmentProfileStore, Event, KernelId, PlatformSettingsService, Reserved, ScreenFactory,
    ScreenModel, ScreenSnapshot, SessionId, SessionPorts, SessionRecord, SessionService,
    SessionSpec, SessionStatus, SessionStore, StoreError, StoredEvent, SubjectId, TerminalGeometry,
};
use layer3_adapter::{
    AdapterError, KernelAdapter, KernelCapabilities, RuntimeHandle, RuntimeSpec, TerminalChannel,
    TerminalInput, TerminalOutput,
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::{self, Message};

const KERNEL: &str = "wire-test";

#[derive(Default)]
struct MemorySessions(Mutex<HashMap<SessionId, SessionRecord>>);

#[async_trait::async_trait]
impl SessionStore for MemorySessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.0
            .lock()
            .await
            .insert(record.session.id.clone(), record.clone());
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.0.lock().await.get(id).cloned())
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        Ok(self
            .0
            .lock()
            .await
            .values()
            .filter(|record| &record.session.owner == owner)
            .cloned()
            .collect())
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        Ok(Reserved::default())
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        Ok(self.0.lock().await.values().cloned().collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.0.lock().await.remove(id);
        Ok(())
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn events_since(
        &self,
        _id: &SessionId,
        _after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct EmptyProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for EmptyProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(&self, _owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct MemoryScreen {
    geometry: TerminalGeometry,
    bytes: Vec<u8>,
}

impl ScreenModel for MemoryScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: Vec::new(),
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

#[derive(Default)]
struct WireAdapter {
    outputs: Mutex<Vec<mpsc::Sender<TerminalOutput>>>,
}

impl WireAdapter {
    async fn output(&self) -> mpsc::Sender<TerminalOutput> {
        self.outputs
            .lock()
            .await
            .last()
            .expect("the session has a terminal channel")
            .clone()
    }

    async fn lose_transport(&self) {
        self.outputs.lock().await.clear();
    }
}

#[async_trait::async_trait]
impl KernelAdapter for WireAdapter {
    fn kernel_name(&self) -> &'static str {
        KERNEL
    }

    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            terminal: true,
            snapshot: true,
            ..KernelCapabilities::default()
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        Ok(RuntimeHandle(format!("runtime-{}", spec.session_id)))
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let (output, output_rx) = mpsc::channel(16);
        let (input, mut input_rx) = mpsc::channel::<TerminalInput>(16);
        self.outputs.lock().await.push(output);
        tokio::spawn(async move { while input_rx.recv().await.is_some() {} });
        Ok(TerminalChannel {
            output: output_rx,
            input,
        })
    }

    async fn refresh_credential(
        &self,
        _handle: &RuntimeHandle,
        _credential: &str,
    ) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn terminate(&self, _handle: &RuntimeHandle) -> Result<(), AdapterError> {
        Ok(())
    }
}

struct Fixture {
    service: Arc<SessionService>,
    adapter: Arc<WireAdapter>,
    caller: Caller,
}

impl Fixture {
    fn new() -> Self {
        let adapter = Arc::new(WireAdapter::default());
        let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
        adapters.insert(KernelId::new(KERNEL), adapter.clone());

        let profiles = EnvironmentProfileService::new(
            Arc::new(EmptyProfiles),
            Baseline::default(),
            None,
            None,
            AgentConfiguration {
                kernel: KernelId::new(KERNEL),
                ..AgentConfiguration::default()
            },
            Arc::new(PlatformSettingsService::unconfigured()),
        );
        let screens: ScreenFactory = Arc::new(|geometry| {
            Box::new(MemoryScreen {
                geometry,
                bytes: Vec::new(),
            })
        });
        let service = SessionService::new(
            adapters,
            screens,
            Arc::new(MemorySessions::default()),
            SessionPorts::default(),
            profiles,
            Deployment::default(),
        );
        let caller = Caller::new(
            SubjectId("wire-owner".into()),
            Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::UseOwnSessions,
            ]),
        );
        Self {
            service,
            adapter,
            caller,
        }
    }

    async fn create_ready_session(&self) -> SessionId {
        let created = self
            .service
            .create_session(
                &self.caller,
                SessionSpec {
                    options: AgentOptions::default(),
                    geometry: Some(TerminalGeometry::default()),
                    environment_profile: None,
                    caller_credential: None,
                    runtime_credential: None,
                    requested_mode: None,
                    quoted_rate_card: None,
                },
            )
            .await
            .expect("the test session is accepted");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let session = self
                    .service
                    .get_session(&self.caller, &created.id)
                    .await
                    .expect("the test session remains readable");
                if session.status == SessionStatus::Ready {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the test session becomes ready");
        created.id
    }
}

struct TestServer(tokio::task::JoinHandle<()>);

impl Drop for TestServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn public_socket(fixture: &Fixture, id: &SessionId) -> (ClientSocket, TestServer) {
    let tickets = layer1_public::tickets::TerminalTickets::new();
    let ticket = tickets
        .issue(&fixture.caller, id, AttachmentClaim::Exclusive)
        .await;
    let app = layer1_public::pty::router(layer1_public::pty::TerminalState {
        sessions: Arc::clone(&fixture.service),
        tickets,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the public test listener binds");
    let address = listener.local_addr().expect("the listener has an address");
    let server = TestServer(tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("the public test router serves");
    }));
    let url = format!(
        "ws://{address}/api/v1/sessions/{}/terminal?ticket={}",
        id.0, ticket.value
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("the public terminal socket opens");
    socket
        .send(Message::Text(
            serde_json::json!({ "t": "resize", "cols": 120, "rows": 40 }).to_string(),
        ))
        .await
        .expect("the required geometry reaches the public router");
    let ready = next_message(&mut socket).await;
    let Message::Text(ready) = ready else {
        panic!("the public socket must announce readiness first: {ready:?}");
    };
    let ready: serde_json::Value = serde_json::from_str(&ready).expect("the ready frame is JSON");
    assert_eq!(ready["t"], "ready");
    (socket, server)
}

async fn next_message(socket: &mut ClientSocket) -> Message {
    tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("the public socket responds in time")
        .expect("the public socket remains present")
        .expect("the public socket frame is readable")
}

#[tokio::test]
async fn public_socket_sends_final_output_then_exit_then_normal_close() {
    let fixture = Fixture::new();
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;
    let runtime = fixture.adapter.output().await;

    runtime
        .send(TerminalOutput::Bytes(b"public-final-output".to_vec()))
        .await
        .expect("the final bytes reach the core");
    runtime
        .send(TerminalOutput::Exited { code: Some(23) })
        .await
        .expect("the status reaches the core");

    assert_eq!(
        next_message(&mut socket).await,
        Message::Binary(b"public-final-output".to_vec())
    );
    let Message::Text(exit) = next_message(&mut socket).await else {
        panic!("the exit report must follow the final output");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&exit).expect("the exit report is JSON"),
        serde_json::json!({ "t": "exit", "code": 23, "reason": "exited" })
    );
    let Message::Close(Some(close)) = next_message(&mut socket).await else {
        panic!("the exit report must be followed by a close frame");
    };
    assert_eq!(
        close.code,
        tungstenite::protocol::frame::coding::CloseCode::Normal
    );
}

#[tokio::test]
async fn public_socket_does_not_invent_exit_or_normal_completion_on_transport_loss() {
    let fixture = Fixture::new();
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;

    fixture.adapter.lose_transport().await;

    let conclusion = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    if serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .is_some_and(|value| value["t"] == "exit")
                    {
                        return "exit";
                    }
                }
                Ok(Message::Close(Some(close)))
                    if close.code == tungstenite::protocol::frame::coding::CloseCode::Normal =>
                {
                    return "normal";
                }
                Ok(Message::Close(_)) | Err(_) => return "unknown",
                Ok(_) => {}
            }
        }
        "unknown"
    })
    .await
    .expect("the lost runtime transport ends the public attachment");
    assert_eq!(
        conclusion, "unknown",
        "transport loss must not become a process conclusion"
    );
}

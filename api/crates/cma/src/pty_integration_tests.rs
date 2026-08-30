//! Public PTY WebSocket contract tests at the composed control-plane boundary.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    AgentConfiguration, AgentOptions, AttachmentClaim, Authorization, Baseline, Caller, Capability,
    ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest,
    ConversationWriteAuthority, Deployment, EnvironmentProfile, EnvironmentProfileId,
    EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentProfileStore, Event,
    HandoverSnapshot, HandoverStore, KernelId, Page, PageCursor, PageRequest,
    PlatformSettingsService, Reserved, ScreenFactory, ScreenModel, ScreenSnapshot, ServiceId,
    SessionId, SessionLease, SessionListQuery, SessionMode, SessionPorts, SessionRecord,
    SessionService, SessionSpec, SessionStatus, SessionStore, StoreError, StoredEvent, SubjectId,
    TerminalGeometry,
};
use layer3_adapter::{
    AdapterError, ConversationChannel, ConversationInputSender, ConversationUpdate, KernelAdapter,
    KernelCapabilities, RuntimeHandle, RuntimeListing, RuntimeOptions, RuntimeSpec,
    TerminalChannel, TerminalInput, TerminalOutput,
};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_tungstenite::tungstenite::{self, Message};

const KERNEL: &str = "wire-test";

#[derive(Default)]
struct MemorySessions {
    state: Mutex<MemorySessionState>,
}

#[derive(Default)]
struct MemorySessionState {
    records: HashMap<SessionId, SessionRecord>,
    conversations: HashMap<SessionId, MemoryConversationState>,
}

#[derive(Default)]
struct MemoryConversationState {
    opening: Option<ConversationOpeningLease>,
    link: Option<ConversationLink>,
    epoch: u64,
}

impl MemorySessionState {
    fn record(&self, id: &SessionId) -> Result<&SessionRecord, StoreError> {
        self.records
            .get(id)
            .ok_or_else(|| StoreError::SessionDeleted { id: id.clone() })
    }

    fn validate_opening_backing(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
    ) -> Result<(), StoreError> {
        let record = self.record(&opening.session_id)?;
        if opening.runtime_generation != 0
            || opening.runtime_candidate_token.is_some()
            || opening.session_lease_token.is_some()
            || session_lease.is_some()
            || record.runtime_id.as_deref() != Some(opening.runtime_id.as_str())
        {
            return Err(StoreError::ConversationOpeningLost {
                id: opening.session_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_authority(&self, authority: &ConversationWriteAuthority) -> Result<(), StoreError> {
        match authority {
            ConversationWriteAuthority::Opening {
                opening,
                session_lease,
            } => {
                self.validate_opening_backing(opening, session_lease.as_ref())?;
                if self
                    .conversations
                    .get(&opening.session_id)
                    .and_then(|state| state.opening.as_ref())
                    != Some(opening)
                {
                    return Err(StoreError::ConversationOpeningLost {
                        id: opening.session_id.clone(),
                    });
                }
            }
            ConversationWriteAuthority::Active(link) => {
                let record = self.record(&link.session_id)?;
                if link.epoch == 0
                    || link.runtime_generation != 0
                    || record.runtime_id.as_deref() != Some(link.runtime_id.as_str())
                    || self
                        .conversations
                        .get(&link.session_id)
                        .and_then(|state| state.link.as_ref())
                        != Some(link)
                {
                    return Err(StoreError::ConversationLinkChanged {
                        id: link.session_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn commit(&mut self, record: &SessionRecord, events: &[Event]) -> Result<u64, StoreError> {
        let revision = record
            .revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        let mut durable = record.clone();
        durable.revision = revision;
        if let Some(last) = events.last() {
            durable.session.last_seq = last.seq;
        }
        self.records.insert(record.session.id.clone(), durable);
        Ok(revision)
    }
}

impl MemorySessions {
    async fn set_runtime_version(&self, id: &SessionId, version: &str) {
        self.state
            .lock()
            .await
            .records
            .get_mut(id)
            .expect("the test session is durable")
            .session
            .runtime_version = Some(version.into());
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.state.lock().await.records.get(id).cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        Ok(page_sessions(
            self.state
                .lock()
                .await
                .records
                .values()
                .filter(|record| &record.session.owner == owner)
                .cloned()
                .collect(),
            query,
        ))
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        Ok(Reserved::default())
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        Ok(page_sessions(
            self.state.lock().await.records.values().cloned().collect(),
            query,
        ))
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        state.records.remove(id);
        state.conversations.remove(id);
        Ok(())
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn acquire_conversation_opening(
        &self,
        request: &ConversationOpeningLeaseRequest,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationOpeningLease, StoreError> {
        let opening = ConversationOpeningLease {
            session_id: request.session_id.clone(),
            holder: request.holder.clone(),
            token: request.token.clone(),
            runtime_id: request.runtime_id.clone(),
            runtime_generation: request.runtime_generation,
            runtime_candidate_token: request.runtime_candidate_token.clone(),
            session_lease_token: session_lease.map(|lease| lease.token.clone()),
        };
        let mut state = self.state.lock().await;
        state.validate_opening_backing(&opening, session_lease)?;
        let conversation = state
            .conversations
            .entry(request.session_id.clone())
            .or_default();
        if let Some(held) = conversation.opening.as_ref() {
            return if held == &opening {
                Ok(opening)
            } else {
                Err(StoreError::ConversationOpeningHeld {
                    id: request.session_id.clone(),
                })
            };
        }
        conversation.opening = Some(opening.clone());
        Ok(opening)
    }

    async fn renew_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
        _ttl: Duration,
    ) -> Result<(), StoreError> {
        let state = self.state.lock().await;
        state.validate_opening_backing(opening, session_lease)?;
        if state
            .conversations
            .get(&opening.session_id)
            .and_then(|state| state.opening.as_ref())
            != Some(opening)
        {
            return Err(StoreError::ConversationOpeningLost {
                id: opening.session_id.clone(),
            });
        }
        Ok(())
    }

    async fn publish_conversation_link(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationLink, StoreError> {
        let mut state = self.state.lock().await;
        state.validate_opening_backing(opening, session_lease)?;
        let conversation = state
            .conversations
            .entry(opening.session_id.clone())
            .or_default();
        if let Some(link) = conversation
            .link
            .as_ref()
            .filter(|link| link.token == opening.token)
        {
            return Ok(link.clone());
        }
        if conversation.opening.as_ref() != Some(opening) {
            return Err(StoreError::ConversationOpeningLost {
                id: opening.session_id.clone(),
            });
        }
        conversation.epoch = conversation
            .epoch
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("conversation link epoch overflow".into()))?;
        let link = ConversationLink {
            session_id: opening.session_id.clone(),
            epoch: conversation.epoch,
            token: opening.token.clone(),
            runtime_generation: opening.runtime_generation,
            runtime_id: opening.runtime_id.clone(),
        };
        conversation.opening = None;
        conversation.link = Some(link.clone());
        Ok(link)
    }

    async fn release_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        let Some(conversation) = state.conversations.get_mut(&opening.session_id) else {
            return Ok(());
        };
        if conversation.opening.as_ref() == Some(opening) {
            conversation.opening = None;
        }
        if conversation.link.as_ref().map(|link| link.token.as_str())
            == Some(opening.token.as_str())
        {
            conversation.link = None;
        }
        Ok(())
    }

    async fn validate_conversation_link(&self, link: &ConversationLink) -> Result<(), StoreError> {
        self.state
            .lock()
            .await
            .validate_authority(&ConversationWriteAuthority::Active(link.clone()))
    }

    async fn commit_session_idempotent(
        &self,
        _write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.state.lock().await.commit(record, events)
    }

    async fn commit_conversation_idempotent(
        &self,
        _write_id: &str,
        authority: &ConversationWriteAuthority,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        let mut state = self.state.lock().await;
        state.validate_authority(authority)?;
        state.commit(record, events)
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.commit_session_idempotent(write_id, record, events)
            .await
    }

    async fn events_since(
        &self,
        _id: &SessionId,
        _after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        Ok(Vec::new())
    }
}

fn page_sessions(mut records: Vec<SessionRecord>, query: &SessionListQuery) -> Page<SessionRecord> {
    records.retain(|record| {
        query
            .status
            .is_none_or(|status| record.session.status == status)
            && query.page.cursor().is_none_or(|cursor| {
                record.session.created_at < cursor.created_at
                    || (record.session.created_at == cursor.created_at
                        && record.session.id.0 < cursor.id)
            })
    });
    records.sort_by(|a, b| {
        b.session
            .created_at
            .cmp(&a.session.created_at)
            .then_with(|| b.session.id.0.cmp(&a.session.id.0))
    });
    let has_more = records.len() > query.page.limit() as usize;
    records.truncate(query.page.limit() as usize);
    let next = has_more.then(|| {
        let last = records.last().expect("a continuing page has a last row");
        PageCursor::new(&last.session.created_at, &last.session.id.0)
    });
    Page {
        items: records,
        next,
    }
}

#[derive(Default)]
struct EmptyProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for EmptyProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(
        &self,
        _owner: &SubjectId,
        _page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn set_preferred(
        &self,
        _owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        if chosen.is_some() {
            return Err(StoreError::Malformed(
                "an empty store has no selectable profile".into(),
            ));
        }
        Ok(())
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
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
    terminal_inputs: Arc<Mutex<Vec<TerminalInput>>>,
    terminal_input_arrived: Arc<Notify>,
    conversations: Mutex<Vec<mpsc::Sender<ConversationUpdate>>>,
    runtimes: Mutex<HashMap<String, String>>,
    credentials: Mutex<Vec<(String, String)>>,
    handovers: Mutex<HashMap<String, HandoverSnapshot>>,
    next_runtime: AtomicUsize,
    pause_handover_readback: AtomicBool,
    handover_readback_started: Notify,
    resume_handover_readback: Notify,
    pause_terminate: AtomicBool,
    terminate_started: Notify,
    resume_terminate: Notify,
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

    async fn wait_until_signal(&self, expected: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let arrived = self.terminal_input_arrived.notified();
                if self.terminal_inputs.lock().await.iter().any(
                    |input| matches!(input, TerminalInput::Signal(signal) if signal == expected),
                ) {
                    break;
                }
                arrived.await;
            }
        })
        .await
        .expect("the signal reaches the runtime while output is blocked");
    }

    fn pause_next_handover_readback(&self) {
        assert!(!self.pause_handover_readback.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_rotation_paused(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.handover_readback_started.notified(),
        )
        .await
        .expect("the rotation reaches its handover readback");
    }

    fn resume_rotation(&self) {
        self.resume_handover_readback.notify_one();
    }

    fn pause_next_termination(&self) {
        assert!(!self.pause_terminate.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_termination_paused(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.terminate_started.notified())
            .await
            .expect("supersession reaches old-runtime termination");
    }

    fn resume_termination(&self) {
        self.resume_terminate.notify_one();
    }
}

#[async_trait::async_trait]
impl HandoverStore for WireAdapter {
    async fn put(&self, id: &SessionId, snapshot: HandoverSnapshot) -> Result<String, StoreError> {
        let reference = format!("sessions/{}/handover.snapshot", id.0);
        self.handovers
            .lock()
            .await
            .insert(reference.clone(), snapshot);
        Ok(reference)
    }

    async fn get_for(
        &self,
        id: &SessionId,
        reference: &str,
    ) -> Result<Option<HandoverSnapshot>, StoreError> {
        let expected = format!("sessions/{}/handover.snapshot", id.0);
        if reference != expected {
            return Err(StoreError::Malformed(
                "handover reference belongs to another session".into(),
            ));
        }
        if self.pause_handover_readback.swap(false, Ordering::SeqCst) {
            // The descriptor is now durable and the input fence is open, but
            // H1 has not yet been superseded. Hold that refusal window open
            // while the established public socket is exercised.
            self.handover_readback_started.notify_one();
            self.resume_handover_readback.notified().await;
        }
        Ok(self.handovers.lock().await.get(reference).cloned())
    }

    async fn delete_for(&self, id: &SessionId) -> Result<(), StoreError> {
        let reference = format!("sessions/{}/handover.snapshot", id.0);
        self.handovers.lock().await.remove(&reference);
        Ok(())
    }
}

#[async_trait::async_trait]
impl KernelAdapter for WireAdapter {
    fn kernel_name(&self) -> &'static str {
        KERNEL
    }

    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            conversation: true,
            terminal: true,
            snapshot: true,
            ..KernelCapabilities::default()
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        let ordinal = self.next_runtime.fetch_add(1, Ordering::SeqCst);
        let handle = RuntimeHandle(format!("runtime-{}-{ordinal}", spec.session_id));
        self.runtimes
            .lock()
            .await
            .insert(handle.0.clone(), spec.session_id.clone());
        Ok(handle)
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let (output, output_rx) = mpsc::channel(16);
        self.outputs.lock().await.push(output);
        let terminal_inputs = Arc::clone(&self.terminal_inputs);
        let terminal_input_arrived = Arc::clone(&self.terminal_input_arrived);
        let input = layer3_adapter::TerminalInputSender::spawn(16, move |input| {
            let terminal_inputs = Arc::clone(&terminal_inputs);
            let terminal_input_arrived = Arc::clone(&terminal_input_arrived);
            async move {
                terminal_inputs.lock().await.push(input);
                terminal_input_arrived.notify_one();
                Ok(())
            }
        });
        Ok(TerminalChannel {
            output: output_rx,
            input,
        })
    }

    async fn open_conversation(
        &self,
        _handle: &RuntimeHandle,
        _options: &RuntimeOptions,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel, AdapterError> {
        self.conversations.lock().await.push(updates);
        Ok(ConversationChannel {
            input: ConversationInputSender::spawn(8, |_input| async { Ok(()) }),
            conversation_id: Some("pty-wire-conversation".into()),
        })
    }

    async fn snapshot(&self, _handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        Ok(Some(b"pty-wire-snapshot".to_vec()))
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &layer3_adapter::RuntimeCredential,
    ) -> Result<(), AdapterError> {
        self.credentials
            .lock()
            .await
            .push((handle.0.clone(), credential.expose().to_string()));
        Ok(())
    }

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        if self.pause_terminate.swap(false, Ordering::SeqCst) {
            // Supersession is broadcast immediately before this call. Holding
            // termination keeps the durable rotation marker present while the
            // accepted socket operation resumes and obtains its typed refusal.
            self.terminate_started.notify_one();
            self.resume_terminate.notified().await;
        }
        self.runtimes.lock().await.remove(&handle.0);
        Ok(())
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        Ok(self
            .runtimes
            .lock()
            .await
            .iter()
            .map(|(handle, session)| RuntimeListing {
                handle: RuntimeHandle(handle.clone()),
                session: Some(session.clone()),
                generation: layer3_adapter::RuntimeGeneration::Legacy,
                created_by: None,
                serving: true,
                lost_because: None,
            })
            .collect())
    }
}

struct Fixture {
    service: Arc<SessionService>,
    adapter: Arc<WireAdapter>,
    store: Arc<MemorySessions>,
    caller: Caller,
}

impl Fixture {
    fn new() -> Self {
        Self::with_runtime_version(None)
    }

    fn with_runtime_version(runtime_version: Option<&str>) -> Self {
        Self::with_baseline(runtime_version, Baseline::default())
    }

    fn with_baseline(runtime_version: Option<&str>, baseline: Baseline) -> Self {
        let adapter = Arc::new(WireAdapter::default());
        let store = Arc::new(MemorySessions::default());
        let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
        adapters.insert(KernelId::new(KERNEL), adapter.clone());

        let profiles = EnvironmentProfileService::new(
            Arc::new(EmptyProfiles),
            baseline,
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
            store.clone(),
            SessionPorts {
                handovers: Some(adapter.clone()),
                ..SessionPorts::default()
            },
            profiles,
            Deployment {
                runtime_version: runtime_version.map(str::to_string),
                ..Deployment::default()
            },
        );
        let caller = Caller::new(
            SubjectId("wire-owner".into()),
            Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::UseOwnSessions,
                Capability::TerminateOwnSession,
            ]),
        );
        Self {
            service,
            adapter,
            store,
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

    async fn wait_until_detached(&self, id: &SessionId) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !self
                    .service
                    .terminal_is_held(&self.caller, id)
                    .await
                    .expect("the test session remains readable")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the disconnected socket releases its attachment");
    }
}

pub(crate) struct CredentialSweepFixture {
    pub(crate) service: Arc<SessionService>,
    pub(crate) session: SessionId,
    pub(crate) owner: SubjectId,
    adapter: Arc<WireAdapter>,
}

impl CredentialSweepFixture {
    pub(crate) async fn delivered_credentials(&self) -> Vec<(String, String)> {
        self.adapter.credentials.lock().await.clone()
    }
}

pub(crate) async fn credential_sweep_fixture() -> CredentialSweepFixture {
    let fixture = Fixture::with_baseline(None, Baseline::new([], [], [ServiceId::new("model")]));
    let session = fixture.create_ready_session().await;
    CredentialSweepFixture {
        service: fixture.service,
        session,
        owner: fixture.caller.subject,
        adapter: fixture.adapter,
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
    let tickets = layer1_public::tickets::TerminalTickets::in_memory();
    let ticket = tickets
        .issue(&fixture.caller, id, AttachmentClaim::Exclusive)
        .await
        .expect("ticket is stored");
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

async fn expect_session_rotating(socket: &mut ClientSocket, operation: &str) {
    let Message::Text(frame) = next_message(socket).await else {
        panic!("{operation} must receive a typed error on the established socket");
    };
    let frame: serde_json::Value =
        serde_json::from_str(&frame).expect("the refusal is a JSON control frame");
    assert_eq!(
        frame["t"], "error",
        "{operation} received the wrong control frame: {frame}"
    );
    assert_eq!(
        frame["code"], "session_rotating",
        "{operation} did not preserve the retryable core refusal: {frame}"
    );
    assert!(
        frame["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "{operation} must carry a useful refusal detail: {frame}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_terminal_output_does_not_stall_signal_or_disconnect_input() {
    let fixture = Fixture::new();
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;
    let outbound = layer1_public::pty::OutboundGate::install(&id);
    let disconnect = layer1_public::pty::DisconnectGate::install(&id);
    let runtime = fixture.adapter.output().await;

    runtime
        .send(TerminalOutput::Bytes(b"blocked-public-output".to_vec()))
        .await
        .expect("terminal output reaches the public writer");
    tokio::time::timeout(Duration::from_secs(2), outbound.wait_until_blocked())
        .await
        .expect("the public writer selects output before the client signal");

    socket
        .send(Message::Text(
            serde_json::json!({ "t": "signal", "sig": "INT" }).to_string(),
        ))
        .await
        .expect("the signal crosses the independently polled socket source");
    fixture.adapter.wait_until_signal("INT").await;

    socket
        .send(Message::Close(None))
        .await
        .expect("the client close reaches the independently polled source");
    tokio::time::timeout(Duration::from_secs(2), disconnect.wait_until_observed())
        .await
        .expect("the input task observes disconnect while the writer remains blocked");

    disconnect.release();
    outbound.release();
    fixture.wait_until_detached(&id).await;
}

#[tokio::test]
async fn established_socket_names_every_operation_refused_during_rotation() {
    let fixture = Fixture::new();
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;
    fixture.adapter.pause_next_handover_readback();

    let mut rotating = tokio::spawn({
        let service = Arc::clone(&fixture.service);
        let caller = fixture.caller.clone();
        let id = id.clone();
        async move {
            service
                .change_plane(&caller, &id, SessionMode::Conversation)
                .await
        }
    });
    tokio::select! {
        _ = fixture.adapter.wait_until_rotation_paused() => {}
        result = &mut rotating => {
            panic!("the plane change ended before publishing Rotating: {result:?}");
        }
    }

    socket
        .send(Message::Binary(b"refused keystroke".to_vec()))
        .await
        .expect("binary input reaches the established socket");
    expect_session_rotating(&mut socket, "binary input").await;

    socket
        .send(Message::Text(
            serde_json::json!({ "t": "resize", "cols": 132, "rows": 44 }).to_string(),
        ))
        .await
        .expect("resize reaches the established socket");
    expect_session_rotating(&mut socket, "resize").await;

    socket
        .send(Message::Text(
            serde_json::json!({ "t": "signal", "sig": "INT" }).to_string(),
        ))
        .await
        .expect("signal reaches the established socket");
    expect_session_rotating(&mut socket, "signal").await;

    fixture.adapter.resume_rotation();
    rotating
        .await
        .expect("the rotation task completes")
        .expect("the rotation continues after refusing old-runtime input");

    let Message::Close(Some(close)) = next_message(&mut socket).await else {
        panic!("the typed refusals must still be followed by the rotation close");
    };
    assert_eq!(u16::from(close.code), 4001);
    assert!(
        close.reason.contains("conversation"),
        "the close still names the plane the session moved to"
    );
}

#[derive(Clone, Copy)]
enum HeldOperation {
    Input,
    Resize,
    Signal,
}

impl HeldOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Resize => "resize",
            Self::Signal => "signal",
        }
    }

    fn frame(self) -> Message {
        match self {
            Self::Input => Message::Binary(b"accepted-before-supersession".to_vec()),
            Self::Resize => Message::Text(
                serde_json::json!({ "t": "resize", "cols": 138, "rows": 46 }).to_string(),
            ),
            Self::Signal => {
                Message::Text(serde_json::json!({ "t": "signal", "sig": "INT" }).to_string())
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RotationEnding {
    PlaneChanged,
    RuntimeReplaced,
}

impl RotationEnding {
    fn close_code(self) -> u16 {
        match self {
            Self::PlaneChanged => 4001,
            Self::RuntimeReplaced => 4002,
        }
    }
}

async fn assert_accepted_operation_precedes_close(
    operation: HeldOperation,
    ending: RotationEnding,
) {
    let fixture = match ending {
        RotationEnding::PlaneChanged => Fixture::new(),
        RotationEnding::RuntimeReplaced => Fixture::with_runtime_version(Some("runtime:new")),
    };
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;
    if matches!(ending, RotationEnding::RuntimeReplaced) {
        // The attach above durably applies its initial geometry. Mark the
        // resulting record old only afterwards so that write cannot restore the
        // current version before reconciliation reads it.
        fixture.store.set_runtime_version(&id, "runtime:old").await;
    }
    let receipt = layer1_public::pty::OperationGate::install(&id, operation.name());

    socket
        .send(operation.frame())
        .await
        .expect("the operation reaches the established socket");
    tokio::time::timeout(Duration::from_secs(2), receipt.wait_until_received())
        .await
        .expect("the server removes the operation from the socket before rotation");

    fixture.adapter.pause_next_termination();
    let rotation = tokio::spawn({
        let service = Arc::clone(&fixture.service);
        let caller = fixture.caller.clone();
        let id = id.clone();
        async move {
            match ending {
                RotationEnding::PlaneChanged => service
                    .change_plane(&caller, &id, SessionMode::Conversation)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                RotationEnding::RuntimeReplaced => {
                    let reclaimed = service.reconcile().await;
                    (reclaimed.rotated_sessions == 1)
                        .then_some(())
                        .ok_or_else(|| format!("expected one runtime rotation, got {reclaimed:?}"))
                }
            }
        }
    });
    fixture.adapter.wait_until_termination_paused().await;

    // Supersession has now been broadcast while the operation selected from the
    // socket is still in flight. Releasing it exercises the precise close race:
    // the sole writer must drain this result before writing the queued ending.
    receipt.release();
    expect_session_rotating(&mut socket, operation.name()).await;
    let Message::Close(Some(close)) = next_message(&mut socket).await else {
        panic!("the drained refusal must be followed by its rotation close");
    };
    assert_eq!(u16::from(close.code), ending.close_code());

    fixture.adapter.resume_termination();
    rotation
        .await
        .expect("the rotation task completes")
        .expect("the rotation continues after the socket drains");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operation_read_before_supersession_is_refused_before_its_close() {
    // Exercise every operation shape accepted on the established socket and
    // both rotation endings. Each case owns a fresh runtime and socket so its
    // gate and close cannot be satisfied by another case.
    for (operation, ending) in [
        (HeldOperation::Input, RotationEnding::PlaneChanged),
        (HeldOperation::Resize, RotationEnding::RuntimeReplaced),
        (HeldOperation::Signal, RotationEnding::PlaneChanged),
    ] {
        assert_accepted_operation_precedes_close(operation, ending).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_queued_behind_the_accepted_operation_stays_outside_core() {
    let fixture = Fixture::new();
    let id = fixture.create_ready_session().await;
    let (mut socket, _server) = public_socket(&fixture, &id).await;
    let accepted = layer1_public::pty::OperationGate::install(&id, "input");
    let queued = layer1_public::pty::OperationGate::install(&id, "signal");

    socket
        .send(Message::Binary(b"accepted-before-ending".to_vec()))
        .await
        .expect("the first operation reaches the established socket");
    tokio::time::timeout(Duration::from_secs(2), accepted.wait_until_received())
        .await
        .expect("the input task accepts exactly the first operation");
    socket
        .send(Message::Text(
            serde_json::json!({ "t": "signal", "sig": "INT" }).to_string(),
        ))
        .await
        .expect("the second operation queues behind the accepted one");

    fixture.adapter.pause_next_termination();
    let rotation = tokio::spawn({
        let service = Arc::clone(&fixture.service);
        let caller = fixture.caller.clone();
        let id = id.clone();
        async move {
            service
                .change_plane(&caller, &id, SessionMode::Conversation)
                .await
        }
    });
    fixture.adapter.wait_until_termination_paused().await;

    accepted.release();
    expect_session_rotating(&mut socket, "input").await;
    let Message::Close(Some(close)) = next_message(&mut socket).await else {
        panic!("the drained operation must be followed by its rotation close");
    };
    assert_eq!(u16::from(close.code), 4001);
    assert!(
        !queued.was_received(),
        "the frame behind an accepted operation must not cross supersession"
    );

    fixture.adapter.resume_termination();
    rotation
        .await
        .expect("the rotation task completes")
        .expect("the plane change completes after the socket drains");
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

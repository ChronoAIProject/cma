//! Production-composition checks for the REST error contract.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::SystemTime,
};

use axum::{
    body::{to_bytes, Body, BodyDataStream},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode},
    Extension, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use layer1_public::{auth::DelegatedCredential, tickets::TerminalTickets, AuthenticatedSubject};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, AgentConfiguration, AgentOptions,
    Authorization, Baseline, Caller, Capability, CatalogError, CatalogService, CatalogSkill,
    EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileSpec,
    EnvironmentProfileStore, Event, EventKind, KernelId, OperationClaim, OperationReservation,
    OperationStore, Page, PageCursor, PageRequest, PlatformSettingsService, RecoveryState,
    RegisteredUser, Reserved, ScreenFactory, ScreenModel, ScreenSnapshot, ServiceCatalog,
    ServiceId, Session, SessionId, SessionListQuery, SessionMode, SessionPorts, SessionRecord,
    SessionService, SessionSpec, SessionStatus, SessionStore, SkillCatalog, SkillId, SkillPack,
    SkillSource, StoreError, StoredEvent, SubjectId, TerminalGeometry, UserDirectory,
    UserDirectoryStore, MAX_PAGE_SIZE,
};
use layer3_adapter::{
    AdapterError, KernelAdapter, KernelCapabilities, RuntimeHandle, RuntimeMode, RuntimeSpec,
    TerminalChannel,
};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tower::ServiceExt;

use super::{rest_api_router, with_cors, with_request_context};
use crate::activity::{self, ActivityRecorder};

const OVERSIZED_BODY_LEN: usize = 2 * 1024 * 1024 + 1;
const LISTED_ORIGIN: &str = "https://integrator.example";
const UNLISTED_ORIGIN: &str = "https://unlisted.example";
const TEST_SERVICE_ID: &str = "api-github";
const TEST_SKILL_NAME: &str = "repository-review";
const TEST_DELEGATED_CREDENTIAL: &str = "contract-delegated-credential";

type OperationIdentity = (String, String, String);
type HeldOperation = ([u8; 32], Option<Vec<u8>>, SystemTime);

#[derive(Default)]
struct MemoryOperations {
    state: Mutex<HashMap<OperationIdentity, HeldOperation>>,
    unavailable: AtomicBool,
    completions_unavailable: AtomicBool,
}

impl MemoryOperations {
    fn identity(claim: &OperationClaim) -> OperationIdentity {
        (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        )
    }
}

#[async_trait::async_trait]
impl OperationStore for MemoryOperations {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable(
                "test operation store is down".into(),
            ));
        }
        let identity = Self::identity(claim);
        let now = SystemTime::now();
        let mut state = self.state.lock().await;
        if state
            .get(&identity)
            .is_some_and(|(_, _, expires_at)| *expires_at <= now)
        {
            state.remove(&identity);
        }
        let Some((fingerprint, outcome, _)) = state.get(&identity) else {
            state.insert(identity, (*claim.fingerprint(), None, claim.expires_at()));
            return Ok(OperationReservation::Acquired);
        };
        if fingerprint != claim.fingerprint() {
            return Ok(OperationReservation::FingerprintMismatch);
        }
        Ok(match outcome {
            Some(outcome) => OperationReservation::Complete(outcome.clone()),
            None => OperationReservation::Pending,
        })
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        if self.unavailable.load(Ordering::SeqCst)
            || self.completions_unavailable.load(Ordering::SeqCst)
        {
            return Err(StoreError::Unavailable(
                "test operation store is down".into(),
            ));
        }
        let mut state = self.state.lock().await;
        let held = state
            .get_mut(&Self::identity(claim))
            .ok_or_else(|| StoreError::Malformed("test operation claim is missing".into()))?;
        if held.2 != claim.expires_at() {
            return Err(StoreError::Malformed(
                "test operation claim was replaced".into(),
            ));
        }
        if &held.0 != claim.fingerprint() {
            return Err(StoreError::Malformed(
                "test operation fingerprint changed".into(),
            ));
        }
        match held.1.as_ref() {
            Some(recorded) if recorded != &outcome => Err(StoreError::Malformed(
                "test operation outcome changed".into(),
            )),
            Some(_) => Ok(()),
            None if held.2 <= SystemTime::now() => Err(StoreError::Malformed(
                "test operation claim expired before completion".into(),
            )),
            None => {
                held.1 = Some(outcome);
                Ok(())
            }
        }
    }
}

#[derive(Default)]
struct MemorySessions {
    state: Mutex<MemorySessionState>,
    fail_lists: AtomicBool,
}

#[derive(Default)]
struct MemorySessionState {
    records: HashMap<String, SessionRecord>,
    events: Vec<Event>,
    receipts: HashMap<String, (SessionId, u64)>,
}

impl MemorySessionState {
    fn event_tail(&self, id: &SessionId) -> u64 {
        self.events
            .iter()
            .filter(|event| &event.session_id == id)
            .map(|event| event.seq)
            .max()
            .unwrap_or(0)
    }

    fn validate_append(
        &self,
        id: &SessionId,
        events: &[Event],
        recorded_tail: u64,
    ) -> Result<u64, StoreError> {
        let mut expected = self.event_tail(id).max(recorded_tail);
        for event in events {
            if &event.session_id != id {
                return Err(StoreError::Malformed(
                    "one event append batch named more than one session".into(),
                ));
            }
            expected = expected.checked_add(1).ok_or_else(|| {
                StoreError::Malformed(format!("event sequence overflow for {id}"))
            })?;
            if event.seq < expected {
                return Err(StoreError::SequenceTaken {
                    expected,
                    got: event.seq,
                });
            }
            if event.seq > expected {
                return Err(StoreError::NonContiguousAppend {
                    expected,
                    got: event.seq,
                });
            }
        }
        Ok(expected)
    }

    fn commit(
        &mut self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
        create: bool,
    ) -> Result<u64, StoreError> {
        if let Some((held_by, revision)) = self.receipts.get(write_id) {
            if held_by != &record.session.id {
                return Err(StoreError::Malformed(format!(
                    "session write ID {write_id} belongs to another session"
                )));
            }
            return Ok(*revision);
        }

        let existing = self.records.get(&record.session.id.0);
        let exists = existing.is_some();
        let actual = existing.map_or(0, |stored| stored.revision);
        let recorded_tail = existing.map_or(0, |stored| stored.session.last_seq);
        let revives_terminal = existing.is_some_and(|stored| {
            stored.session.status.is_terminal() && !record.session.status.is_terminal()
        });
        if create {
            if record.revision != 0 || exists {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual,
                });
            }
        } else {
            if !exists {
                return Err(StoreError::SessionDeleted {
                    id: record.session.id.clone(),
                });
            }
            if record.revision != actual {
                return Err(StoreError::RevisionConflict {
                    expected: record.revision,
                    actual,
                });
            }
            if revives_terminal {
                return Err(StoreError::SessionTerminated {
                    id: record.session.id.clone(),
                });
            }
        }

        let durable_tail = self.validate_append(&record.session.id, events, recorded_tail)?;
        let tail_matches = if events.is_empty() {
            record.session.last_seq <= durable_tail
        } else {
            record.session.last_seq == durable_tail
        };
        if !tail_matches {
            return Err(StoreError::Malformed(format!(
                "session {} record tail {} does not match durable tail {}",
                record.session.id, record.session.last_seq, durable_tail
            )));
        }

        let revision = actual
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
        let mut durable = record.clone();
        durable.revision = revision;
        durable.session.last_seq = durable_tail;
        self.events.extend_from_slice(events);
        self.records.insert(record.session.id.0.clone(), durable);
        self.receipts
            .insert(write_id.to_string(), (record.session.id.clone(), revision));
        Ok(revision)
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.state.lock().await.records.get(&id.0).cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        if self.fail_lists.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
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
        let mut reserved = Reserved::default();
        for record in self.state.lock().await.records.values() {
            if record.session.status.wants_runtime() {
                reserved.include(record.session.options.size);
            }
        }
        Ok(reserved)
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        if self.fail_lists.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(page_sessions(
            self.state.lock().await.records.values().cloned().collect(),
            query,
        ))
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let mut state = self.state.lock().await;
        state.records.remove(&id.0);
        state.events.retain(|event| &event.session_id != id);
        state.receipts.retain(|_, (session_id, _)| session_id != id);
        Ok(())
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        let Some(first) = events.first() else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        let recorded_tail = state
            .records
            .get(&first.session_id.0)
            .map_or(0, |record| record.session.last_seq);
        state.validate_append(&first.session_id, events, recorded_tail)?;
        state.events.extend_from_slice(events);
        Ok(())
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.state
            .lock()
            .await
            .commit(write_id, record, events, false)
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.state
            .lock()
            .await
            .commit(write_id, record, events, true)
    }

    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        Ok(self.state.lock().await.event_tail(id))
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let mut events: Vec<_> = self
            .state
            .lock()
            .await
            .events
            .iter()
            .filter(|event| &event.session_id == id && event.seq > after)
            .cloned()
            .map(StoredEvent::from)
            .collect();
        events.sort_by_key(|event| event.seq);
        Ok(events)
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

struct FirstProfilePagePause {
    reached: Semaphore,
    resume: Semaphore,
}

impl FirstProfilePagePause {
    fn new() -> Self {
        Self {
            reached: Semaphore::new(0),
            resume: Semaphore::new(0),
        }
    }
}

#[derive(Default)]
struct MemoryProfiles {
    records: Mutex<HashMap<String, EnvironmentProfile>>,
    list_calls: AtomicUsize,
    list_limits: Mutex<Vec<u32>>,
    preference_writes: AtomicUsize,
    first_page_pause: Mutex<Option<Arc<FirstProfilePagePause>>>,
}

#[async_trait::async_trait]
impl EnvironmentProfileStore for MemoryProfiles {
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError> {
        self.records
            .lock()
            .await
            .insert(profile.id.0.clone(), profile.clone());
        Ok(())
    }

    async fn replace_content(
        &self,
        id: &EnvironmentProfileId,
        content: &EnvironmentProfileSpec,
        updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        let mut records = self.records.lock().await;
        let Some(profile) = records.get_mut(&id.0) else {
            return Ok(None);
        };
        profile.apply_content(content, updated_at);
        Ok(Some(profile.clone()))
    }

    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(self.records.lock().await.get(&id.0).cloned())
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.list_limits.lock().await.push(page.limit());
        let listed = page_profiles(
            self.records
                .lock()
                .await
                .values()
                .filter(|profile| &profile.owner == owner)
                .cloned()
                .collect(),
            page,
        );
        let pause = if page.cursor().is_none() {
            self.first_page_pause.lock().await.take()
        } else {
            None
        };
        if let Some(pause) = pause {
            pause.reached.add_permits(1);
            pause
                .resume
                .acquire()
                .await
                .expect("the test still owns the page pause")
                .forget();
        }
        Ok(listed)
    }

    async fn set_preferred(
        &self,
        owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        let mut records = self.records.lock().await;
        if let Some(id) = chosen {
            let selected = records.get(&id.0).filter(|profile| &profile.owner == owner);
            if selected.is_none() {
                return Err(StoreError::Malformed(
                    "selected environment profile is not owned by the subject".into(),
                ));
            }
        }
        for profile in records
            .values_mut()
            .filter(|profile| &profile.owner == owner)
        {
            profile.preferred = chosen.is_some_and(|id| profile.id == *id);
        }
        self.preference_writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn preferred_for(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(self
            .records
            .lock()
            .await
            .values()
            .filter(|profile| &profile.owner == owner && profile.preferred)
            .max_by(|a, b| {
                a.updated_at
                    .cmp(&b.updated_at)
                    .then_with(|| a.id.0.cmp(&b.id.0))
            })
            .cloned())
    }

    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError> {
        self.records.lock().await.remove(&id.0);
        Ok(())
    }
}

fn page_profiles(
    mut profiles: Vec<EnvironmentProfile>,
    page: &PageRequest,
) -> Page<EnvironmentProfile> {
    profiles.retain(|profile| {
        page.cursor().is_none_or(|cursor| {
            profile.created_at < cursor.created_at
                || (profile.created_at == cursor.created_at && profile.id.0 < cursor.id)
        })
    });
    profiles.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.0.cmp(&a.id.0))
    });
    let has_more = profiles.len() > page.limit() as usize;
    profiles.truncate(page.limit() as usize);
    let next = has_more.then(|| {
        let last = profiles.last().expect("a continuing page has a last row");
        PageCursor::new(&last.created_at, &last.id.0)
    });
    Page {
        items: profiles,
        next,
    }
}

#[derive(Default)]
struct TestDirectory {
    unavailable: AtomicBool,
}

#[async_trait::async_trait]
impl UserDirectoryStore for TestDirectory {
    async fn record_sign_in(&self, _user: &RegisteredUser) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(
        &self,
        _query: &layer2_core::DirectoryQuery,
    ) -> Result<Vec<RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }

    async fn by_subjects(&self, subjects: &[SubjectId]) -> Result<Vec<RegisteredUser>, StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("test directory is down".into()));
        }
        Ok(subjects
            .iter()
            .cloned()
            .map(|subject| RegisteredUser {
                subject,
                display_name: Some("Contract User".into()),
                email: None,
                avatar_url: Some("https://images.example.test/contract-user.png".into()),
                first_seen: "2026-08-24T00:00:00Z".into(),
                last_seen: "2026-08-24T00:00:00Z".into(),
            })
            .collect())
    }
}

#[derive(Default)]
struct TestCatalogue {
    unavailable: AtomicBool,
    service_reads: AtomicUsize,
    skill_reads: AtomicUsize,
    service_credentials: Mutex<Vec<Option<String>>>,
    skill_credentials: Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl ServiceCatalog for TestCatalogue {
    async fn available(
        &self,
        _subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogService>, CatalogError> {
        self.service_reads.fetch_add(1, Ordering::SeqCst);
        self.service_credentials
            .lock()
            .await
            .push(credential.map(str::to_owned));
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(CatalogError::Unavailable("test catalogue is down".into()));
        }
        Ok(vec![CatalogService {
            id: ServiceId::new(TEST_SERVICE_ID),
            name: "GitHub".into(),
            description: Some("Repositories and pull requests".into()),
        }])
    }
}

#[async_trait::async_trait]
impl SkillCatalog for TestCatalogue {
    async fn available(
        &self,
        _subject: &SubjectId,
        credential: Option<&str>,
    ) -> Result<Vec<CatalogSkill>, CatalogError> {
        self.skill_reads.fetch_add(1, Ordering::SeqCst);
        self.skill_credentials
            .lock()
            .await
            .push(credential.map(str::to_owned));
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(CatalogError::Unavailable("test catalogue is down".into()));
        }
        Ok(vec![CatalogSkill {
            id: SkillId::new("skl_repository_review"),
            name: TEST_SKILL_NAME.into(),
            description: "Review a repository without changing it".into(),
            access: Some("private".into()),
        }])
    }
}

struct TestSkills;

#[async_trait::async_trait]
impl SkillSource for TestSkills {
    async fn packs_for(
        &self,
        _caller_credential: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Vec<SkillPack>, String> {
        Ok(skills
            .iter()
            .map(|skill| SkillPack {
                name: skill.0.clone(),
                version: Some("1.2.3".into()),
                files: vec![("SKILL.md".into(), b"contract fixture".to_vec())],
            })
            .collect())
    }
}

struct NoActivity;

#[async_trait::async_trait]
impl layer2_core::ActivityStore for NoActivity {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(&self, _query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(Vec::new())
    }
}

struct EmptyScreen(TerminalGeometry);

impl ScreenModel for EmptyScreen {
    fn apply(&mut self, _bytes: &[u8]) {}

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.0 = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.0,
            rows: Vec::new(),
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[derive(Default)]
struct TestKernel {
    conversation_updates: Mutex<Option<mpsc::Sender<layer3_adapter::ConversationUpdate>>>,
    resource_samples: Mutex<Option<mpsc::Sender<layer3_adapter::ResourceSample>>>,
    provisions: AtomicUsize,
    conversation_inputs: Arc<AtomicUsize>,
}

impl TestKernel {
    async fn emit_message_chunk(&self, text: &str) {
        let updates = self
            .conversation_updates
            .lock()
            .await
            .clone()
            .expect("the conversation fixture is connected");
        updates
            .send(layer3_adapter::ConversationUpdate::Said(
                layer3_adapter::SessionUpdate::AgentMessageChunk(
                    layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(text)),
                ),
            ))
            .await
            .expect("the conversation fixture is listening");
    }

    async fn emit_resource_sample(&self, sample: layer3_adapter::ResourceSample) {
        let readings = self
            .resource_samples
            .lock()
            .await
            .clone()
            .expect("the resource fixture is connected");
        readings
            .send(sample)
            .await
            .expect("the resource fixture is listening");
    }
}

#[async_trait::async_trait]
impl KernelAdapter for TestKernel {
    fn kernel_name(&self) -> &'static str {
        "rest-contract-test"
    }

    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            terminal: true,
            conversation: true,
            snapshot: true,
            resources: true,
            ..Default::default()
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        self.provisions.fetch_add(1, Ordering::SeqCst);
        Ok(RuntimeHandle(format!("runtime-{}", spec.session_id)))
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let (input, mut received) = mpsc::channel(8);
        let (output, produced) = mpsc::channel(8);
        tokio::spawn(async move {
            let _keep_output_open = output;
            while received.recv().await.is_some() {}
        });
        Ok(TerminalChannel {
            output: produced,
            input,
        })
    }

    async fn open_conversation(
        &self,
        _handle: &RuntimeHandle,
        _options: &layer3_adapter::RuntimeOptions,
        updates: mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> Result<layer3_adapter::ConversationChannel, AdapterError> {
        *self.conversation_updates.lock().await = Some(updates);
        let received = Arc::clone(&self.conversation_inputs);
        Ok(layer3_adapter::ConversationChannel {
            input: layer3_adapter::ConversationInputSender::spawn(8, move |_input| {
                let received = Arc::clone(&received);
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }),
            conversation_id: Some("rest-contract-conversation".into()),
        })
    }

    async fn refresh_credential(
        &self,
        _handle: &RuntimeHandle,
        _credential: &str,
    ) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn snapshot(&self, _handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        Ok(Some(b"rest-contract-conversation".to_vec()))
    }

    async fn watch_resources(
        &self,
        _handle: &RuntimeHandle,
        readings: mpsc::Sender<layer3_adapter::ResourceSample>,
    ) -> Result<(), AdapterError> {
        *self.resource_samples.lock().await = Some(readings);
        Ok(())
    }

    async fn switch_plane(
        &self,
        _handle: &RuntimeHandle,
        _mode: RuntimeMode,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::Rejected(
            "the REST fixture requires runtime carry-over".into(),
        ))
    }

    async fn terminate(&self, _handle: &RuntimeHandle) -> Result<(), AdapterError> {
        Ok(())
    }
}

struct TestSurface {
    app: Router,
    sessions: Arc<SessionService>,
    tickets: Arc<TerminalTickets>,
    kernel: Arc<TestKernel>,
    store: Arc<MemorySessions>,
    operations: Arc<MemoryOperations>,
    profiles: Arc<MemoryProfiles>,
    catalogue: Arc<TestCatalogue>,
    directory: Arc<TestDirectory>,
    caller: Caller,
}

fn test_surface(authenticated: bool) -> TestSurface {
    let settings = Arc::new(PlatformSettingsService::unconfigured());
    let catalogue = Arc::new(TestCatalogue::default());
    let profiles = Arc::new(MemoryProfiles::default());
    let environments = EnvironmentProfileService::new(
        Arc::clone(&profiles) as Arc<dyn EnvironmentProfileStore>,
        Baseline::new(
            [],
            [SkillId::new(TEST_SKILL_NAME)],
            [ServiceId::new(TEST_SERVICE_ID)],
        ),
        Some(Arc::clone(&catalogue) as Arc<dyn SkillCatalog>),
        Some(Arc::clone(&catalogue) as Arc<dyn ServiceCatalog>),
        AgentConfiguration {
            kernel: KernelId::new("test"),
            ..Default::default()
        },
        Arc::clone(&settings),
    );
    let kernel = Arc::new(TestKernel::default());
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(
        KernelId::new("test"),
        Arc::clone(&kernel) as Arc<dyn KernelAdapter>,
    );
    let factory: ScreenFactory = Arc::new(|geometry| Box::new(EmptyScreen(geometry)));
    let store = Arc::new(MemorySessions::default());
    let operations = Arc::new(MemoryOperations::default());
    let sessions = SessionService::new(
        adapters,
        factory,
        Arc::clone(&store) as Arc<dyn SessionStore>,
        SessionPorts {
            skills: Some(Arc::new(TestSkills)),
            settings: Some(Arc::clone(&settings)),
            ..Default::default()
        },
        Arc::clone(&environments),
        Default::default(),
    );
    let directory = Arc::new(TestDirectory::default());
    let users = UserDirectory::new(Arc::clone(&directory) as Arc<dyn UserDirectoryStore>);
    let activity = ActivityService::new(Arc::new(NoActivity));
    let caller = Caller::new(
        SubjectId("subject-contract".into()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ReadAllSessions,
            Capability::ManageOwnEnvironmentProfiles,
            Capability::ReadUserDirectory,
            Capability::ReadActivityLog,
            Capability::ConfigurePlatform,
        ]),
    );
    let tickets = TerminalTickets::in_memory();
    let public_state = layer1_public::rest::SessionsState {
        sessions: Arc::clone(&sessions),
        users: Arc::clone(&users),
        tickets: Arc::clone(&tickets),
        environments: Arc::clone(&environments),
        operations: Arc::clone(&operations) as Arc<dyn OperationStore>,
    };
    let rest = rest_api_router(
        public_state.clone(),
        Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&sessions),
            activity: Arc::clone(&activity),
            users,
            settings,
        }),
    );
    let app = Router::new()
        .merge(rest)
        .merge(layer1_public::agui::router(public_state))
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(activity),
            activity::record,
        ));
    let app = if authenticated {
        app.layer(Extension(caller.authorization.clone()))
            .layer(Extension(AuthenticatedSubject(caller.subject.clone())))
            .layer(Extension(DelegatedCredential::subject(
                TEST_DELEGATED_CREDENTIAL.into(),
            )))
    } else {
        app
    };
    TestSurface {
        app,
        sessions,
        tickets,
        kernel,
        store,
        operations,
        profiles,
        catalogue,
        directory,
        caller,
    }
}

fn json_request(method: Method, uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .expect("request builds")
}

fn keyed_json_request(
    method: Method,
    uri: &str,
    key: &str,
    body: impl Into<Body>,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", key)
        .body(body.into())
        .expect("request builds")
}

fn preflight_request(
    uri: &str,
    origin: &'static str,
    requested_method: Method,
    requested_headers: Option<&'static str>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .header(header::ORIGIN, origin)
        .header(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            requested_method.as_str(),
        );
    if let Some(requested_headers) = requested_headers {
        request = request.header(header::ACCESS_CONTROL_REQUEST_HEADERS, requested_headers);
    }
    request
        .body(Body::empty())
        .expect("preflight request builds")
}

fn assert_header_list_contains(headers: &HeaderMap, name: HeaderName, expected: &str) {
    let value = headers
        .get(&name)
        .unwrap_or_else(|| panic!("response is missing {name}"))
        .to_str()
        .unwrap_or_else(|_| panic!("{name} is not text"));
    assert!(
        value
            .split(',')
            .any(|item| item.trim().eq_ignore_ascii_case(expected)),
        "{name} did not contain {expected:?}: {value:?}"
    );
}

async fn assert_json(response: Response<Body>, status: StatusCode) -> serde_json::Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static("application/json"))
    );
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("JSON responses are bounded"),
    )
    .expect("the response is JSON")
}

async fn assert_problem(response: Response<Body>, status: StatusCode) -> serde_json::Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        ))
    );
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small"),
    )
    .expect("the problem document is json");
    assert_eq!(body["status"], status.as_u16());
    assert!(
        body["code"].as_str().is_some_and(|code| !code.is_empty()),
        "problem code must be non-empty: {body}"
    );
    body
}

async fn assert_request_too_large(response: Response<Body>) {
    let body = assert_problem(response, StatusCode::PAYLOAD_TOO_LARGE).await;
    assert_eq!(body["code"], "request_too_large");
    assert_eq!(
        body["detail"],
        "The request body exceeds what this service accepts."
    );
}

async fn create_session(app: &Router) -> (SessionId, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(json_request(Method::POST, "/api/v1/sessions", "{}"))
        .await
        .expect("router answers");
    let body = assert_json(response, StatusCode::CREATED).await;
    let id = SessionId(body["id"].as_str().expect("session id").to_string());
    (id, body)
}

async fn wait_until_ready(sessions: &SessionService, caller: &Caller, id: &SessionId) {
    for _ in 0..200 {
        let session = sessions
            .get_session(caller, id)
            .await
            .expect("the created session is readable");
        if session.status == SessionStatus::Ready {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the in-memory runtime did not become ready");
}

async fn create_ready_session(
    app: &Router,
    sessions: &SessionService,
    caller: &Caller,
) -> SessionId {
    let (id, _) = create_session(app).await;
    wait_until_ready(sessions, caller, &id).await;
    id
}

fn session_record(sequence: usize, owner: &SubjectId, status: SessionStatus) -> SessionRecord {
    SessionRecord::new(Session {
        id: SessionId(format!("sesn_page_{sequence:04}")),
        owner: owner.clone(),
        kernel: KernelId::new("test"),
        status,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        // Every fixture deliberately ties on time, so traversal depends on
        // the durable id tie-breaker instead of passing accidentally.
        created_at: "2026-08-24T10:11:12Z".into(),
    })
}

async fn seed_sessions(
    store: &MemorySessions,
    owner: &SubjectId,
    statuses: impl IntoIterator<Item = SessionStatus>,
) {
    let records = statuses
        .into_iter()
        .enumerate()
        .map(|(sequence, status)| session_record(sequence, owner, status));
    let mut state = store.state.lock().await;
    for record in records {
        state.records.insert(record.session.id.0.clone(), record);
    }
}

async fn create_ready_conversation_session(
    sessions: &Arc<SessionService>,
    caller: &Caller,
) -> SessionId {
    let session = sessions
        .create_session(
            caller,
            SessionSpec {
                options: Default::default(),
                geometry: None,
                environment_profile: None,
                caller_credential: Some(TEST_DELEGATED_CREDENTIAL.into()),
                runtime_credential: None,
                requested_mode: Some(SessionMode::Conversation),
                quoted_rate_card: None,
            },
        )
        .await
        .expect("the conversation session is accepted");
    wait_until_ready(sessions, caller, &session.id).await;
    session.id
}

#[path = "agui_auth_contract_tests.rs"]
mod agui_auth_contract_tests;

async fn next_agui_frame(body: &mut BodyDataStream) -> serde_json::Value {
    loop {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), body.next())
            .await
            .expect("the SSE stream emits within the test deadline")
            .expect("the SSE stream stays open")
            .expect("the SSE body is readable");
        let raw = String::from_utf8(bytes.to_vec()).expect("SSE framing is text");
        let data = raw
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() && raw.lines().any(|line| line.starts_with(':')) {
            continue;
        }
        assert!(
            !data.is_empty(),
            "an AG-UI event frame must carry SSE data: {raw:?}"
        );
        return serde_json::from_str(&data)
            .unwrap_or_else(|error| panic!("AG-UI data is JSON ({error}): {raw:?}"));
    }
}

fn assert_agui_ids(frame: &serde_json::Value, run_id: &str, thread_id: &SessionId) {
    assert_eq!(frame["runId"], run_id, "unexpected run context: {frame}");
    assert_eq!(
        frame["threadId"], thread_id.0,
        "unexpected session context: {frame}"
    );
}

fn profile_record(sequence: usize, owner: &SubjectId) -> EnvironmentProfile {
    EnvironmentProfile {
        preferred: false,
        id: EnvironmentProfileId(format!("envp_page_{sequence:04}")),
        owner: owner.clone(),
        name: format!("profile {sequence}"),
        variables: Vec::new(),
        packages: Vec::new(),
        services: Vec::new(),
        agent_notes: String::new(),
        agent: AgentConfiguration {
            kernel: KernelId::new("test"),
            ..Default::default()
        },
        created_at: "2026-08-24T10:11:12Z".into(),
        updated_at: "2026-08-24T10:11:12Z".into(),
    }
}

async fn seed_profiles(store: &MemoryProfiles, owner: &SubjectId, count: usize) {
    let mut stored = store.records.lock().await;
    for sequence in 0..count {
        let profile = profile_record(sequence, owner);
        stored.insert(profile.id.0.clone(), profile);
    }
}

fn next_link(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::LINK).iter().find_map(|value| {
        value
            .to_str()
            .expect("Link is text")
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix(">; rel=\"next\""))
            .map(str::to_string)
    })
}

async fn get_collection(app: &Router, uri: &str) -> (Vec<serde_json::Value>, HeaderMap) {
    let response = app
        .clone()
        .oneshot(
            Request::get(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK, "for {uri}");
    let headers = response.headers().clone();
    let body = assert_json(response, StatusCode::OK).await;
    let rows = body
        .as_array()
        .cloned()
        .expect("collection body is an array");
    (rows, headers)
}

async fn seed_cursor_contract_events(store: &MemorySessions, id: &SessionId) {
    store.state.lock().await.events = [41, 42, 43]
        .into_iter()
        .map(|seq| Event {
            session_id: id.clone(),
            seq,
            kind: EventKind::SessionCreated,
            appended_at: String::new(),
        })
        .collect();
}

async fn initial_sse_frames(app: &Router, uri: &str, last_event_id: Option<&str>) -> String {
    let mut request = Request::get(uri);
    if let Some(last_event_id) = last_event_id {
        request = request.header("last-event-id", last_event_id);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("request builds"))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK, "for {uri}");
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static("text/event-stream")),
        "for {uri}"
    );

    let mut data = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), data.next())
        .await
        .expect("the seeded replay answers without polling")
        .expect("the replay contains one frame")
        .expect("the SSE body is readable");
    let mut rendered = String::from_utf8(first.to_vec()).expect("SSE frames are text");
    while let Ok(Some(next)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), data.next()).await
    {
        let bytes = next.expect("the SSE body is readable");
        rendered.push_str(std::str::from_utf8(&bytes).expect("SSE frames are text"));
    }
    rendered
}

fn assert_creator(view: &serde_json::Value) {
    assert_eq!(
        view["creator"],
        serde_json::json!({
            "display_name": "Contract User",
            "avatar_url": "https://images.example.test/contract-user.png"
        })
    );
}

fn assert_creator_absent(view: &serde_json::Value) {
    assert!(
        view.get("creator").is_none(),
        "an unavailable directory omits the creator: {view}"
    );
}

async fn assert_catalogue_received_credential(
    catalogue: &TestCatalogue,
    route: &str,
    expect_skills: bool,
) {
    let services = std::mem::take(&mut *catalogue.service_credentials.lock().await);
    assert!(
        !services.is_empty(),
        "{route} must project the service catalogue"
    );
    assert!(
        services
            .iter()
            .all(|credential| credential.as_deref() == Some(TEST_DELEGATED_CREDENTIAL)),
        "{route} must pass the delegated credential to service projection: {services:?}"
    );

    let skills = std::mem::take(&mut *catalogue.skill_credentials.lock().await);
    if expect_skills {
        assert!(
            !skills.is_empty(),
            "{route} must project the skill catalogue"
        );
        assert!(
            skills
                .iter()
                .all(|credential| credential.as_deref() == Some(TEST_DELEGATED_CREDENTIAL)),
            "{route} must pass the delegated credential to skill projection: {skills:?}"
        );
    } else {
        assert!(
            skills.is_empty(),
            "{route} has no recorded skills to project: {skills:?}"
        );
    }
}

fn assert_named_reach(view: &serde_json::Value) {
    assert_eq!(
        view["reach"],
        serde_json::json!([{
            "id": TEST_SERVICE_ID,
            "name": "GitHub",
            "description": "Repositories and pull requests",
            "mandated": true
        }])
    );
}

fn assert_unnamed_reach(view: &serde_json::Value) {
    assert_eq!(
        view["reach"],
        serde_json::json!([{
            "id": TEST_SERVICE_ID,
            "mandated": true
        }])
    );
}

fn assert_named_skills(view: &serde_json::Value) {
    assert_eq!(
        view["agent"]["skills"],
        serde_json::json!([{
            "id": TEST_SKILL_NAME,
            "version": "1.2.3",
            "name": TEST_SKILL_NAME,
            "description": "Review a repository without changing it"
        }])
    );
}

fn assert_unnamed_skills(view: &serde_json::Value) {
    assert_eq!(
        view["agent"]["skills"],
        serde_json::json!([{
            "id": TEST_SKILL_NAME,
            "version": "1.2.3"
        }])
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn replay_frames_use_the_session_id_for_initial_history_and_the_followed_tail() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let before_turn = sessions
        .events_since(&caller, &id, None)
        .await
        .expect("the owner can read the session")
        .last()
        .map(|event| event.seq)
        .expect("creation records events");

    sessions
        .send_prompt(&caller, &id, "already in history".into())
        .await
        .expect("the real session accepts the opening turn");
    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id={}&starting_after={before_turn}",
                id.0
            ))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let mut initial_types = Vec::new();
    for _ in 0..4 {
        let frame = next_agui_frame(&mut body).await;
        assert_agui_ids(&frame, &id.0, &id);
        initial_types.push(
            frame["type"]
                .as_str()
                .expect("AG-UI frames name their type")
                .to_string(),
        );
    }
    assert_eq!(
        initial_types,
        [
            "TEXT_MESSAGE_START",
            "TEXT_MESSAGE_CONTENT",
            "TEXT_MESSAGE_END",
            "RUN_STARTED",
        ],
        "the initial read must exercise the already-recorded turn"
    );

    sessions
        .terminate_session(&caller, &id)
        .await
        .expect("terminating the real session records a followed event");
    let followed = next_agui_frame(&mut body).await;
    assert_eq!(followed["type"], "RUN_ERROR", "the tail was not followed");
    assert_agui_ids(&followed, &id.0, &id);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn followed_live_and_resource_frames_use_the_session_id_for_both_contexts() {
    let TestSurface {
        app,
        sessions,
        kernel,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let head = sessions
        .events_since(&caller, &id, None)
        .await
        .expect("the owner can read the session")
        .last()
        .map(|event| event.seq)
        .expect("creation records events");
    let response = app
        .oneshot(
            Request::get(format!(
                "/api/v1/agui/events?thread_id={}&starting_after={head}",
                id.0
            ))
            .body(Body::empty())
            .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    kernel.emit_message_chunk("being typed").await;
    let live = next_agui_frame(&mut body).await;
    assert_eq!(live["type"], "TEXT_MESSAGE_CONTENT");
    assert_eq!(live["cma:live"], true);
    assert!(
        live.get("seq").is_none(),
        "live frames carry no cursor: {live}"
    );
    assert_agui_ids(&live, &id.0, &id);

    kernel
        .emit_resource_sample(layer3_adapter::ResourceSample {
            cpu_millis_used: Some(250),
            cpu_millis_limit: Some(1_000),
            memory_bytes_used: Some(128),
            memory_bytes_limit: Some(256),
            memory_bytes_peak: Some(192),
            stall_permille: None,
            ceiling_hits: Some(1),
            allocation_failures: Some(0),
            kills: Some(0),
            received_bytes: Some(512),
            sent_bytes: Some(1_024),
            received_bytes_per_second: Some(64),
            sent_bytes_per_second: Some(128),
            interval_ms: Some(1_000),
        })
        .await;
    let resources = next_agui_frame(&mut body).await;
    assert_eq!(resources["type"], "STATE_DELTA");
    assert_eq!(resources["cma:live"], true);
    assert!(
        resources.get("seq").is_none(),
        "resource frames carry no cursor: {resources}"
    );
    assert_eq!(resources["delta"][0]["path"], "/resources");
    assert_eq!(resources["delta"][0]["value"]["memoryBytesUsed"], 128);
    assert_agui_ids(&resources, &id.0, &id);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn run_frames_echo_the_explicit_client_run_id_instead_of_the_session_id() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let run_id = "run_explicit_contract";
    assert_ne!(run_id, id.0);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/agui/run",
            serde_json::json!({
                "threadId": id.0,
                "runId": run_id,
                "messages": [{ "role": "user", "content": "echo my run id" }]
            })
            .to_string(),
        ))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    for _ in 0..4 {
        let frame = next_agui_frame(&mut body).await;
        assert_agui_ids(&frame, run_id, &id);
    }
}

async fn assert_server_defaulted_run_id(explicit_null: bool) {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let mut request = serde_json::json!({
        "threadId": id.0,
        "messages": [{ "role": "user", "content": "choose the run id" }]
    });
    if explicit_null {
        request
            .as_object_mut()
            .expect("the request fixture is an object")
            .insert("runId".into(), serde_json::Value::Null);
    }

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/agui/run",
            request.to_string(),
        ))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let first = next_agui_frame(&mut body).await;
    let generated = first["runId"]
        .as_str()
        .expect("a generated run id is a string")
        .to_string();
    let suffix = generated
        .strip_prefix("run_")
        .expect("generated run ids use the documented prefix");
    assert_eq!(suffix.len(), 32, "the generated UUID is unhyphenated");
    assert!(
        suffix
            .chars()
            .all(|character| character.is_ascii_hexdigit()),
        "the generated UUID is hexadecimal: {generated}"
    );
    assert_eq!(suffix, suffix.to_ascii_lowercase());
    assert_ne!(generated, id.0, "a run response is not a replay stream");
    assert_agui_ids(&first, &generated, &id);

    for _ in 1..4 {
        let frame = next_agui_frame(&mut body).await;
        assert_agui_ids(&frame, &generated, &id);
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_omitted_run_id_is_generated_once_and_echoed_on_every_run_frame() {
    assert_server_defaulted_run_id(false).await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_null_run_id_uses_the_same_server_default_as_an_omitted_id() {
    assert_server_defaulted_run_id(true).await;
}

async fn invalid_run_id_problem(supplied: impl FnOnce(&SessionId) -> String) -> serde_json::Value {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    let before = sessions
        .events_since(&caller, &id, None)
        .await
        .expect("the owner can read the session before the refusal")
        .last()
        .map(|event| event.seq);
    let run_id = supplied(&id);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/agui/run",
            serde_json::json!({
                "threadId": id.0,
                "runId": run_id,
                "messages": [{ "role": "user", "content": "must not be sent" }]
            })
            .to_string(),
        ))
        .await
        .expect("router answers");
    let problem = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    let after = sessions
        .events_since(&caller, &id, None)
        .await
        .expect("the owner can read the session after the refusal")
        .last()
        .map(|event| event.seq);
    assert_eq!(after, before, "an invalid run id must not open a turn");
    problem
}

#[tokio::test]
async fn a_run_id_equal_to_the_effective_thread_id_is_a_typed_client_refusal() {
    let problem = invalid_run_id_problem(|id| id.0.clone()).await;
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(
        problem["detail"],
        "invalid request: runId must differ from the effective threadId"
    );
}

#[tokio::test]
async fn an_empty_or_whitespace_run_id_is_a_typed_client_refusal() {
    for supplied in ["", " \t"] {
        let problem = invalid_run_id_problem(|_| supplied.to_string()).await;
        assert_eq!(problem["code"], "invalid_request", "for {supplied:?}");
        assert_eq!(
            problem["detail"], "invalid request: runId must contain a non-whitespace character",
            "for {supplied:?}"
        );
    }
}

#[tokio::test]
async fn listed_origin_completes_an_authorized_http_cors_sequence() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("loopback listener binds");
    let address = listener.local_addr().expect("listener has an address");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let client = reqwest::Client::new();
    let response = client
        .request(Method::OPTIONS, format!("http://{address}/api/v1/sessions"))
        .header(header::ORIGIN, LISTED_ORIGIN)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type,idempotency-key",
        )
        .send()
        .await
        .expect("the HTTP server answers the preflight");
    let status = response.status();
    let headers = response.headers().clone();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("listed origin is allowed"),
        LISTED_ORIGIN
    );
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .expect("credential mode is retained"),
        "true"
    );
    assert_header_list_contains(
        &headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "authorization",
    );
    assert_header_list_contains(
        &headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "content-type",
    );
    assert_header_list_contains(
        &headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "idempotency-key",
    );
    assert_header_list_contains(&headers, header::ACCESS_CONTROL_ALLOW_METHODS, "POST");
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_MAX_AGE)
            .expect("preflight decisions are cacheable"),
        "3600"
    );

    let response = client
        .post(format!("http://{address}/api/v1/sessions"))
        .header(header::ORIGIN, LISTED_ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer browser-fixture")
        .header("idempotency-key", "browser-create")
        .body("{}")
        .send()
        .await
        .expect("the admitted browser request reaches the HTTP server");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("the actual response retains the browser grant"),
        LISTED_ORIGIN
    );
    let body = response
        .json::<serde_json::Value>()
        .await
        .expect("the authenticated create response is JSON");
    assert!(
        body["id"].is_string(),
        "the authenticated call created no session: {body}"
    );

    shutdown_tx.send(()).expect("the server is still running");
    server
        .await
        .expect("the server task joins")
        .expect("the server stops cleanly");
}

#[tokio::test]
async fn listed_origin_allows_the_sdk_reconnect_cursor_header() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/sessions/session/events?starting_after=17",
            LISTED_ORIGIN,
            Method::GET,
            Some("authorization,last-event-id"),
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "authorization",
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "last-event-id",
    );
}

#[tokio::test]
async fn listed_origin_can_read_the_pagination_link_header() {
    let surface = test_surface(true);
    seed_sessions(
        &surface.store,
        &surface.caller.subject,
        [SessionStatus::Ready, SessionStatus::Ready],
    )
    .await;
    let app = with_cors(surface.app, vec![HeaderValue::from_static(LISTED_ORIGIN)]);
    let response = app
        .oneshot(
            Request::get("/api/v1/sessions?limit=1")
                .header(header::ORIGIN, LISTED_ORIGIN)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        "link",
    );
    assert!(next_link(response.headers()).is_some());
}

#[tokio::test]
async fn rest_event_cursor_spellings_each_resume_at_the_next_event_once() {
    let TestSurface {
        app,
        sessions,
        store,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;
    seed_cursor_contract_events(&store, &id).await;

    for query in ["starting_after=42", "after=42"] {
        let uri = format!("/api/v1/sessions/{}/events?{query}", id.0);
        let frame = initial_sse_frames(&app, &uri, None).await;
        assert_eq!(frame.matches("id: 43").count(), 1, "for {query}: {frame:?}");
        assert!(frame.contains("\"seq\":43"), "for {query}: {frame:?}");
        assert!(
            frame.contains(&format!("\"session_id\":\"{}\"", id.0)),
            "for {query}: {frame:?}"
        );
        assert!(frame.contains("\"event\":"), "for {query}: {frame:?}");
        assert!(
            frame.lines().all(|line| !line.starts_with("event:")),
            "REST uses SSE's default event type: {frame:?}"
        );
        assert!(!frame.contains("id: 41"), "for {query}: {frame:?}");
        assert!(!frame.contains("id: 42"), "for {query}: {frame:?}");
    }
}

#[tokio::test]
async fn last_event_id_wins_over_a_conflicting_rest_query_cursor() {
    let TestSurface {
        app,
        sessions,
        store,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;
    seed_cursor_contract_events(&store, &id).await;

    let uri = format!("/api/v1/sessions/{}/events?starting_after=41", id.0);
    let frame = initial_sse_frames(&app, &uri, Some("42")).await;
    assert_eq!(frame.matches("id: 43").count(), 1, "{frame:?}");
    assert!(!frame.contains("id: 42"), "{frame:?}");
}

#[tokio::test]
async fn listed_origin_allows_profile_put_preflight() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/environment-profiles/x",
            LISTED_ORIGIN,
            Method::PUT,
            None,
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "PUT",
    );
}

#[tokio::test]
async fn unlisted_origin_receives_no_cors_grant() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    for (path, method, requested_headers) in [
        ("/api/v1/sessions", Method::POST, "authorization"),
        (
            "/api/v1/agui/events?thread_id=sesn_cross_origin",
            Method::GET,
            "authorization,last-event-id",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(preflight_request(
                path,
                UNLISTED_ORIGIN,
                method,
                Some(requested_headers),
            ))
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::OK, "for {path}");
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "an unlisted origin must not receive a browser grant for {path}"
        );
    }
}

#[tokio::test]
async fn every_json_route_rejects_malformed_bodies_as_problem_documents() {
    let app = test_surface(true).app;
    let routes = [
        (Method::POST, "/api/v1/sessions"),
        (Method::POST, "/api/v1/sessions/sesn_bad/input"),
        (Method::POST, "/api/v1/sessions/sesn_bad/resize"),
        (
            Method::POST,
            "/api/v1/sessions/sesn_bad/approvals/request_bad",
        ),
        (Method::POST, "/api/v1/sessions/sesn_bad/steer"),
        (Method::POST, "/api/v1/sessions/sesn_bad/plane"),
        (Method::POST, "/api/v1/sessions/sesn_bad/terminal-ticket"),
        (Method::POST, "/api/v1/environment-profiles"),
        (Method::PUT, "/api/v1/environment-profiles/preferred"),
        (Method::PUT, "/api/v1/environment-profiles/envp_bad"),
        (Method::PATCH, "/api/v1/admin/settings"),
        (Method::PUT, "/api/v1/admin/settings"),
        (Method::POST, "/api/v1/admin/settings/reset"),
    ];

    for (method, uri) in routes {
        let response = app
            .clone()
            .oneshot(json_request(method.clone(), uri, "{"))
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "malformed_json", "for {method} {uri}");
    }
}

#[tokio::test]
async fn bad_query_strings_are_problem_documents_on_every_query_route() {
    let app = test_surface(true).app;
    for uri in [
        "/api/v1/sessions/sesn_bad/events?after=not-a-number",
        "/api/v1/sessions/sesn_bad/events?starting_after=not-a-number",
        "/api/v1/sessions/sesn_bad/events?startingafter=17",
        "/api/v1/sessions/sesn_bad/events?starting_after=17&starting_after=18",
        "/api/v1/sessions/sesn_bad/events?after=17&after=18",
        "/api/v1/sessions/sesn_bad/events?starting_after=17&after=17",
        "/api/v1/pricing/quote?environment_profile=one&environment_profile=two",
        "/api/v1/sessions?limitt=5",
        "/api/v1/sessions?cursor=not-base64url",
        "/api/v1/admin/sessions?limitt=5",
        "/api/v1/environment-profiles?limitt=5",
        "/api/v1/environment-profiles?cursor=not-base64url",
        "/api/v1/admin/users?limit=not-a-number",
        "/api/v1/admin/activity?limit=not-a-number",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "invalid_query", "for {uri}");
    }
}

#[tokio::test]
async fn cursors_reject_cross_route_reuse_truncation_and_nonsemantic_timestamps() {
    let TestSurface {
        app,
        store,
        profiles,
        caller,
        ..
    } = test_surface(true);
    seed_sessions(
        &store,
        &caller.subject,
        [SessionStatus::Ready, SessionStatus::Ready],
    )
    .await;
    seed_profiles(&profiles, &caller.subject, 2).await;

    let (_, headers) = get_collection(&app, "/api/v1/sessions?limit=1").await;
    let next = next_link(&headers).expect("two sessions produce a continuation");
    let query = next
        .split_once('?')
        .map(|(_, query)| query)
        .expect("continuation carries a query");

    for uri in [
        format!("/api/v1/admin/sessions?{query}"),
        format!("/api/v1/environment-profiles?{query}"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(&uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "invalid_query", "for {uri}");
    }

    let (before_cursor, encoded) = query
        .split_once("cursor=")
        .expect("continuation names its cursor");
    let mut truncated = encoded.to_string();
    truncated.pop();
    let truncated_uri = format!("/api/v1/sessions?{before_cursor}cursor={truncated}");
    let response = app
        .clone()
        .oneshot(
            Request::get(&truncated_uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
    assert_eq!(body["code"], "invalid_query");

    let invalid_time = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "collection": "sessions",
            "created_at": "newest-ish",
            "id": "sesn_cursor",
        }))
        .expect("test cursor serializes"),
    );
    let response = app
        .oneshot(
            Request::get(format!("/api/v1/sessions?cursor={invalid_time}"))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
    assert_eq!(body["code"], "invalid_query");
}

#[tokio::test]
async fn a_wrong_json_media_type_is_itself_a_problem_document() {
    let response = test_surface(true)
        .app
        .oneshot(
            Request::post("/api/v1/sessions")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    assert_eq!(body["code"], "unsupported_media_type");
}

#[tokio::test]
async fn a_schema_error_never_reflects_the_submitted_value() {
    let planted = "not-a-real-secret-value";
    let body = format!(r#"{{"name":"profile","variables":"{planted}"}}"#);
    let response = test_surface(true)
        .app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/environment-profiles",
            body,
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "invalid_request");
    let rendered = serde_json::to_string(&body).expect("problem serializes");
    assert!(
        !rendered.contains(planted),
        "problem reflected input: {body}"
    );
    assert_eq!(
        body["detail"],
        "The JSON body does not match this operation's request schema."
    );
}

#[tokio::test]
async fn the_optional_ticket_body_distinguishes_empty_from_untyped() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;
    let uri = format!("/api/v1/sessions/{}/terminal-ticket", id.0);

    let response = app
        .clone()
        .oneshot(
            Request::post(&uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let ticket: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("ticket responses are bounded"),
    )
    .expect("the ticket is json");
    assert!(ticket["ticket"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    let response = app
        .clone()
        .oneshot(
            Request::post(&uri)
                .body(Body::from(r#"{"takeover":true}"#))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    assert_eq!(body["code"], "unsupported_media_type");

    let response = app
        .clone()
        .oneshot(json_request(Method::POST, &uri, "null"))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "invalid_request");

    let planted = "not-a-real-ticket-secret";
    let response = app
        .oneshot(json_request(
            Method::POST,
            &uri,
            format!(r#"{{"takeover":"{planted}"}}"#),
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "invalid_request");
    assert!(!serde_json::to_string(&body)
        .expect("problem serializes")
        .contains(planted));
}

#[tokio::test]
async fn an_authenticated_route_without_a_credential_returns_the_coded_401() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
    assert_eq!(body["code"], "not_authenticated");
}

#[tokio::test]
async fn kernel_discovery_without_a_credential_returns_the_coded_401() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/kernels")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
    assert_eq!(body["code"], "not_authenticated");
    assert_eq!(
        body["type"],
        "https://spec.chrono-ai.fun/cma/1.0/problems/not-authenticated"
    );
}

#[tokio::test]
async fn authenticated_kernel_discovery_returns_the_deployment_offering() {
    let response = test_surface(true)
        .app
        .oneshot(
            Request::get("/api/v1/kernels")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static("application/json"))
    );
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("kernel offerings are bounded"),
    )
    .expect("kernel offerings are json");
    assert_eq!(
        body,
        serde_json::json!({
            "kernels": [{ "id": "test", "modes": ["terminal", "conversation"] }]
        })
    );
}

#[tokio::test]
async fn conformance_remains_public() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/conformance")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("conformance documents are bounded"),
    )
    .expect("the conformance document is json");
    assert!(
        body["conformsTo"]
            .as_array()
            .is_some_and(|classes| !classes.is_empty()),
        "public conformance document must name at least one class: {body}"
    );
    assert_eq!(
        body["version"],
        option_env!("CHRONO_BUILD").unwrap_or("development")
    );
    assert!(
        body["build_sha"]
            .as_str()
            .is_some_and(|build| !build.is_empty()),
        "conformance must identify the running build: {body}"
    );
}

#[tokio::test]
async fn openapi_is_public_typed_and_covers_the_complete_rest_surface() {
    let app = test_surface(false).app;
    let response = app
        .clone()
        .oneshot(
            Request::get("/openapi.json")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let document = assert_json(response, StatusCode::OK).await;
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(
        document["info"]["version"],
        option_env!("CHRONO_BUILD").unwrap_or("development")
    );

    let paths = document["paths"]
        .as_object()
        .expect("OpenAPI paths are an object");
    let expected: std::collections::BTreeSet<_> = [
        "/openapi.json",
        "/api/v1/pricing/quote",
        "/api/v1/sessions",
        "/api/v1/sessions/allowance",
        "/api/v1/sessions/{id}",
        "/api/v1/sessions/{id}/terminate",
        "/api/v1/sessions/{id}/stop",
        "/api/v1/sessions/{id}/plane",
        "/api/v1/sessions/{id}/revive",
        "/api/v1/sessions/{id}/terminal-ticket",
        "/api/v1/sessions/{id}/input",
        "/api/v1/sessions/{id}/interrupt",
        "/api/v1/sessions/{id}/steer",
        "/api/v1/sessions/{id}/resize",
        "/api/v1/sessions/{id}/approvals/{request_id}",
        "/api/v1/sessions/{id}/usage",
        "/api/v1/sessions/{id}/resources",
        "/api/v1/sessions/{id}/spend",
        "/api/v1/sessions/{id}/screen",
        "/api/v1/sessions/{id}/transcript",
        "/api/v1/sessions/{id}/events",
        "/api/v1/conformance",
        "/api/v1/kernels",
        "/api/v1/environment-profiles",
        "/api/v1/environment-profiles/skills",
        "/api/v1/environment-profiles/services",
        "/api/v1/environment-profiles/{id}",
        "/api/v1/environment-profiles/preferred",
        "/api/v1/admin/users",
        "/api/v1/admin/activity",
        "/api/v1/admin/runtimes",
        "/api/v1/admin/sessions",
        "/api/v1/admin/sessions/{id}/stop",
        "/api/v1/admin/sessions/{id}/terminate",
        "/api/v1/admin/settings",
        "/api/v1/admin/settings/reset",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        paths
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );

    let event_parameters = paths["/api/v1/sessions/{id}/events"]["get"]["parameters"]
        .as_array()
        .expect("the event operation declares its parameters");
    assert!(
        event_parameters
            .iter()
            .any(|parameter| parameter["name"] == "starting_after"),
        "the canonical event cursor is absent from OpenAPI: {event_parameters:?}"
    );
    assert!(
        event_parameters
            .iter()
            .all(|parameter| parameter["name"] != "after"),
        "the compatibility cursor alias must not be advertised: {event_parameters:?}"
    );

    let mut keyed_operations = Vec::new();
    for (path, path_item) in paths {
        for method in ["get", "post", "put", "patch", "delete"] {
            let Some(parameters) = path_item[method]["parameters"].as_array() else {
                continue;
            };
            if parameters
                .iter()
                .any(|parameter| parameter["name"] == "Idempotency-Key")
            {
                keyed_operations.push(format!("{method} {path}"));
            }
        }
    }
    keyed_operations.sort();
    assert_eq!(
        keyed_operations,
        ["post /api/v1/sessions", "post /api/v1/sessions/{id}/input",],
        "only the two deduplicated operations may advertise Idempotency-Key"
    );

    let discovery_schema =
        &paths["/openapi.json"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(
        discovery_schema["$ref"], "#/components/schemas/OpenApiDocument",
        "the discovery response must describe its JSON document: {discovery_schema}"
    );

    let transcript_schema = &paths["/api/v1/sessions/{id}/transcript"]["get"]["responses"]["200"]
        ["content"]["application/octet-stream"]["schema"];
    assert_eq!(
        transcript_schema["$ref"], "#/components/schemas/TranscriptBody",
        "the transcript response must reference its binary schema: {transcript_schema}"
    );

    let ticket_body = &paths["/api/v1/sessions/{id}/terminal-ticket"]["post"]["requestBody"];
    assert_eq!(
        ticket_body["required"], false,
        "the ticket body may be omitted: {ticket_body}"
    );
    let ticket_schema = &ticket_body["content"]["application/json"]["schema"];
    assert_eq!(
        ticket_schema["$ref"], "#/components/schemas/TicketRequest",
        "an explicit ticket body must be a non-null TicketRequest: {ticket_schema}"
    );
    let ticket_responses = &paths["/api/v1/sessions/{id}/terminal-ticket"]["post"]["responses"];
    assert_eq!(
        ticket_responses["503"]["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/ProblemDocument",
        "ticket-store failures must advertise their typed problem: {ticket_responses}"
    );
    assert_eq!(
        ticket_responses["default"]["$ref"], "#/components/responses/Problem",
        "the shared OpenAPI problem fallback must remain present: {ticket_responses}"
    );

    let profile_create_schema = &paths["/api/v1/environment-profiles"]["post"]["requestBody"]
        ["content"]["application/json"]["schema"];
    assert_eq!(
        profile_create_schema,
        &serde_json::json!({ "$ref": "#/components/schemas/ProfileRequest" }),
        "profile creation accepts only ProfileRequest: {profile_create_schema}"
    );
    let profile_replacement_schema = &paths["/api/v1/environment-profiles/{id}"]["put"]
        ["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(
        profile_replacement_schema,
        &serde_json::json!({
            "oneOf": [
                { "$ref": "#/components/schemas/ProfileRequest" },
                { "$ref": "#/components/schemas/ProfileView" }
            ]
        }),
        "profile replacement accepts the writable request or a complete view: {profile_replacement_schema}"
    );

    let problem = &document["components"]["schemas"]["ProblemDocument"];
    for required in ["type", "title", "status", "detail", "code"] {
        assert!(
            problem["properties"].get(required).is_some(),
            "problem schema is missing {required}: {problem}"
        );
        assert!(
            problem["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "problem schema does not require {required}: {problem}"
        );
    }
    for optional in ["field", "path"] {
        assert!(
            problem["properties"].get(optional).is_some(),
            "problem schema is missing the emitted {optional} extension: {problem}"
        );
        assert!(
            !problem["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == optional)),
            "problem extension {optional} must remain optional: {problem}"
        );
    }
    assert!(
        problem["properties"].get("request_id").is_none(),
        "request correlation belongs in the X-Request-Id header, not the problem body: {problem}"
    );
    assert_eq!(
        document["components"]["responses"]["Problem"]["content"]["application/problem+json"]
            ["schema"]["$ref"],
        "#/components/schemas/ProblemDocument"
    );
    assert_eq!(
        document["components"]["responses"]["Problem"]["headers"]["X-Request-Id"]["schema"]["type"],
        "string"
    );
    assert_eq!(
        document["components"]["responses"]["Problem"]["headers"]["Link"]["schema"]["type"],
        "string"
    );

    fn assert_local_refs_resolve(value: &serde_json::Value, document: &serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
                    let pointer = reference
                        .strip_prefix('#')
                        .expect("the generated document uses only local references");
                    assert!(
                        document.pointer(pointer).is_some(),
                        "OpenAPI reference does not resolve: {reference}"
                    );
                }
                for nested in object.values() {
                    assert_local_refs_resolve(nested, document);
                }
            }
            serde_json::Value::Array(array) => {
                for nested in array {
                    assert_local_refs_resolve(nested, document);
                }
            }
            _ => {}
        }
    }
    assert_local_refs_resolve(&document, &document);

    fn schema_allows_null(schema: &serde_json::Value) -> bool {
        schema["nullable"] == true
            || schema["type"] == "null"
            || schema["type"]
                .as_array()
                .is_some_and(|types| types.iter().any(|kind| kind == "null"))
            || ["oneOf", "anyOf"].iter().any(|composition| {
                schema[*composition]
                    .as_array()
                    .is_some_and(|members| members.iter().any(schema_allows_null))
            })
    }

    fn schema_has_minimum_one(schema: &serde_json::Value) -> bool {
        schema["minimum"].as_u64() == Some(1)
            || ["oneOf", "anyOf"].iter().any(|composition| {
                schema[*composition]
                    .as_array()
                    .is_some_and(|members| members.iter().any(schema_has_minimum_one))
            })
    }

    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("component schemas are an object");

    fn assert_exact_schema_properties(
        schemas: &serde_json::Map<String, serde_json::Value>,
        schema: &str,
        expected: &[&str],
    ) {
        let properties = schemas
            .get(schema)
            .unwrap_or_else(|| panic!("OpenAPI is missing the {schema} schema"))["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{schema} properties are an object"));
        let actual = properties
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let expected = expected
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual, expected, "{schema} fields must remain exact");
    }

    for (schema, expected_properties) in [
        (
            "ProfileRequest",
            &[
                "name",
                "variables",
                "packages",
                "agent",
                "nyxid",
                "agent_notes",
            ][..],
        ),
        (
            "ProfileView",
            &[
                "id",
                "name",
                "variables",
                "packages",
                "agent",
                "nyxid",
                "agent_notes",
                "mandatory",
                "created_at",
                "updated_at",
                "preferred",
            ][..],
        ),
        (
            "AgentBody",
            &[
                "kernel",
                "mode",
                "model",
                "reasoning_effort",
                "permission",
                "credential",
                "size",
                "record_tool_bodies",
                "skills",
            ][..],
        ),
        (
            "ProfileAgentView",
            &[
                "kernel",
                "mode",
                "model",
                "reasoning_effort",
                "permission",
                "credential",
                "size",
                "record_tool_bodies",
                "skills",
            ][..],
        ),
        ("VariableBody", &["key", "value"][..]),
        ("PackageBody", &["name", "version"][..]),
        ("NyxIdBody", &["services"][..]),
        ("NyxIdView", &["services"][..]),
        ("SizeBody", &["cpu_millis", "memory_mib"][..]),
        ("SessionSize", &["cpu_millis", "memory_mib"][..]),
        ("MandatoryView", &["packages", "skills"][..]),
    ] {
        assert_exact_schema_properties(schemas, schema, expected_properties);
    }

    fn assert_only_schema_ref(schema: &serde_json::Value, expected: &str) {
        fn collect_refs<'a>(
            value: &'a serde_json::Value,
            refs: &mut std::collections::BTreeSet<&'a str>,
        ) {
            match value {
                serde_json::Value::Object(object) => {
                    if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str)
                    {
                        refs.insert(reference);
                    }
                    for nested in object.values() {
                        collect_refs(nested, refs);
                    }
                }
                serde_json::Value::Array(array) => {
                    for nested in array {
                        collect_refs(nested, refs);
                    }
                }
                _ => {}
            }
        }

        let mut actual = std::collections::BTreeSet::new();
        collect_refs(schema, &mut actual);
        assert_eq!(
            actual,
            std::collections::BTreeSet::from([expected]),
            "schema must reference only {expected}: {schema}"
        );
    }

    for (schema, property, reference) in [
        (
            "ProfileRequest",
            "variables",
            "#/components/schemas/VariableBody",
        ),
        (
            "ProfileRequest",
            "packages",
            "#/components/schemas/PackageBody",
        ),
        ("ProfileRequest", "agent", "#/components/schemas/AgentBody"),
        ("ProfileRequest", "nyxid", "#/components/schemas/NyxIdBody"),
        (
            "ProfileView",
            "variables",
            "#/components/schemas/VariableBody",
        ),
        (
            "ProfileView",
            "packages",
            "#/components/schemas/PackageBody",
        ),
        (
            "ProfileView",
            "agent",
            "#/components/schemas/ProfileAgentView",
        ),
        ("ProfileView", "nyxid", "#/components/schemas/NyxIdView"),
        (
            "ProfileView",
            "mandatory",
            "#/components/schemas/MandatoryView",
        ),
        ("AgentBody", "size", "#/components/schemas/SizeBody"),
        (
            "ProfileAgentView",
            "size",
            "#/components/schemas/SessionSize",
        ),
    ] {
        assert_only_schema_ref(&schemas[schema]["properties"][property], reference);
    }

    let discovery_document = &schemas["OpenApiDocument"];
    for required in ["openapi", "info", "paths"] {
        assert!(
            discovery_document["properties"].get(required).is_some(),
            "OpenAPI discovery schema is missing {required}: {discovery_document}"
        );
    }

    let transcript_body = &schemas["TranscriptBody"];
    assert_eq!(transcript_body["type"], "string");
    assert_eq!(transcript_body["format"], "binary");
    assert!(
        transcript_body.get("items").is_none(),
        "the raw transcript must not be modeled as an integer array: {transcript_body}"
    );

    let change_request = &schemas["ChangeRequest"];
    for optional in [
        "briefing",
        "deployment_capacity",
        "size_bounds",
        "live_sessions",
    ] {
        assert!(
            !change_request["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|field| field == optional)),
            "PATCH property {optional} must remain optional: {change_request}"
        );
    }
    for non_nullable in ["briefing", "deployment_capacity", "size_bounds"] {
        let property = &change_request["properties"][non_nullable];
        assert!(
            !schema_allows_null(property),
            "PATCH property {non_nullable} must reject null: {property}"
        );
    }
    let live_sessions = &change_request["properties"]["live_sessions"];
    assert!(
        schema_allows_null(live_sessions),
        "live_sessions null removes the optional limit: {live_sessions}"
    );
    assert!(
        schema_has_minimum_one(live_sessions),
        "a non-null live_sessions limit must be positive: {live_sessions}"
    );
    for (schema, expected_properties) in [
        (
            "SessionView",
            &["id", "status", "agent", "conforms_to", "reach"][..],
        ),
        (
            "ActivityView",
            &["id", "at", "action", "target", "outcome", "detail"][..],
        ),
        (
            "PlatformSettings",
            &[
                "briefing",
                "deployment_capacity",
                "memory_pressure",
                "pricing",
                "size_bounds",
                "live_sessions",
            ][..],
        ),
        (
            "MemoryPressurePolicy",
            &[
                "enter_permille",
                "exit_permille",
                "enter_samples",
                "exit_samples",
                "settle_seconds",
            ][..],
        ),
    ] {
        let schema = schemas
            .get(schema)
            .unwrap_or_else(|| panic!("OpenAPI is missing the {schema} schema"));
        for property in expected_properties {
            assert!(
                schema["properties"].get(property).is_some(),
                "{schema} is missing {property}: {schema}"
            );
        }
    }
    assert!(
        schemas.contains_key("ActivityTarget"),
        "the tagged activity target must remain typed"
    );
    for (schema, expected_properties) in [
        (
            "SessionAgentView",
            &["permission", "reasoning_effort", "credential", "skills"][..],
        ),
        (
            "SessionSkillView",
            &["id", "version", "name", "description"][..],
        ),
        (
            "CatalogSkillView",
            &["id", "name", "description", "access"][..],
        ),
    ] {
        let schema_properties = schemas
            .get(schema)
            .unwrap_or_else(|| panic!("OpenAPI is missing the distinct {schema} schema"))
            ["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{schema} properties are an object"));
        for property in expected_properties {
            assert!(
                schema_properties.contains_key(*property),
                "{schema} is missing {property}"
            );
        }
    }
    assert_eq!(
        schemas["SessionView"]["properties"]["agent"]["$ref"],
        "#/components/schemas/SessionAgentView"
    );
    assert_eq!(
        schemas["ProfileView"]["properties"]["agent"]["$ref"],
        "#/components/schemas/ProfileAgentView"
    );
    assert_eq!(
        schemas["InputRequest"]["properties"]["newline"]["default"], true,
        "terminal input must advertise its runtime newline default"
    );

    let mut operations = 0;
    let mut operation_ids = std::collections::BTreeSet::new();
    for (template, item) in paths {
        for method in ["get", "put", "post", "delete", "patch"] {
            if item.get(method).is_none() {
                continue;
            }
            operations += 1;
            let operation = &item[method];
            let operation_id = operation["operationId"]
                .as_str()
                .expect("every SDK operation has an identifier");
            assert!(
                operation_ids.insert(operation_id),
                "duplicate OpenAPI operationId: {operation_id}"
            );
            assert_eq!(
                operation["responses"]["default"]["$ref"], "#/components/responses/Problem",
                "{method} {template} must type its refusals"
            );
            assert!(
                operation["parameters"]
                    .as_array()
                    .is_some_and(|parameters| parameters.iter().any(|parameter| {
                        parameter["name"] == "X-Request-Id"
                            && parameter["in"] == "header"
                            && parameter["required"] == false
                    })),
                "{method} {template} must document caller correlation"
            );
            for (status, response) in operation["responses"]
                .as_object()
                .expect("operation responses are an object")
            {
                if status == "default" {
                    continue;
                }
                assert_eq!(
                    response["headers"]["X-Request-Id"]["schema"]["type"], "string",
                    "{method} {template} {status} must document correlation"
                );
                assert_eq!(
                    response["headers"]["Link"]["schema"]["type"], "string",
                    "{method} {template} {status} must document discovery links"
                );
            }
            let path = template
                .replace("{id}", "sesn_openapi_parity")
                .replace("{request_id}", "approval_openapi_parity");
            let request = Request::builder()
                .method(
                    Method::from_bytes(method.to_ascii_uppercase().as_bytes())
                        .expect("OpenAPI method is HTTP"),
                )
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("parity request builds");
            let response = app.clone().oneshot(request).await.expect("router answers");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "documented {method} {template} has no route"
            );
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "documented {method} {template} has no matching method"
            );
        }
    }
    assert_eq!(operations, 43, "every REST operation must be documented");
}

#[tokio::test]
async fn request_ids_cover_success_auth_and_routing_failures_and_echo_callers() {
    let app = with_request_context(test_surface(false).app);
    for (uri, status) in [
        ("/api/v1/conformance", StatusCode::OK),
        ("/api/v1/sessions", StatusCode::UNAUTHORIZED),
        ("/api/v1/no-such-rest-route", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), status, "for {uri}");
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok());
        assert!(
            request_id.is_some_and(|value| !value.is_empty()),
            "{uri} must carry a minted request id"
        );
    }

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/conformance")
                .header("x-request-id", "support-case-488")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.headers().get("x-request-id"),
        Some(&HeaderValue::from_static("support-case-488"))
    );

    for unusable in [
        HeaderValue::from_static(""),
        HeaderValue::from_bytes(b" \t ").expect("whitespace is a valid header value"),
        HeaderValue::from_bytes(&[0xff]).expect("opaque bytes are a valid header value"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/conformance")
                    .header("x-request-id", unusable.clone())
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let generated = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("an unusable request id is replaced with text");
        assert_ne!(
            response.headers().get("x-request-id"),
            Some(&unusable),
            "the unusable caller value must not be echoed"
        );
        assert!(
            uuid::Uuid::parse_str(generated).is_ok(),
            "the replacement must be a generated UUID: {generated:?}"
        );
    }
}

#[tokio::test]
async fn rest_responses_advertise_profile_and_service_description_links() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/conformance")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let links = response
        .headers()
        .get_all(header::LINK)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(links.contains("rel=\"profile\""), "{links}");
    assert!(
        links.contains("</openapi.json>; rel=\"service-desc\""),
        "{links}"
    );
}

#[tokio::test]
async fn session_list_distinguishes_empty_success_from_a_store_outage() {
    let TestSurface {
        app,
        sessions,
        store,
        caller,
        ..
    } = test_surface(true);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("session lists are bounded"),
    )
    .expect("the session list is json");
    assert_eq!(body, serde_json::json!([]));

    let id = create_ready_session(&app, &sessions, &caller).await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("session lists are bounded"),
    )
    .expect("the session list is json");
    assert_eq!(body.as_array().map(Vec::len), Some(1));
    assert_eq!(body[0]["id"], id.0);

    store.fail_lists.store(true, Ordering::SeqCst);
    let response = app
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(body["code"], "runtime_unavailable");
    assert!(body["type"]
        .as_str()
        .is_some_and(|problem_type| problem_type.ends_with("/runtime-unavailable")));
}

#[tokio::test]
async fn session_list_defaults_to_fifty_and_clamps_both_limit_bounds() {
    let TestSurface {
        app, store, caller, ..
    } = test_surface(true);
    seed_sessions(
        &store,
        &caller.subject,
        std::iter::repeat_n(SessionStatus::Ready, 205),
    )
    .await;

    let (default_page, default_headers) = get_collection(&app, "/api/v1/sessions").await;
    assert_eq!(default_page.len(), 50);
    assert!(next_link(&default_headers).is_some());

    let (maximum_page, maximum_headers) = get_collection(&app, "/api/v1/sessions?limit=500").await;
    assert_eq!(maximum_page.len(), 200);
    assert!(next_link(&maximum_headers)
        .expect("205 rows continue after the maximum page")
        .contains("limit=200&cursor="));

    let (minimum_page, minimum_headers) = get_collection(&app, "/api/v1/sessions?limit=0").await;
    assert_eq!(minimum_page.len(), 1);
    assert!(next_link(&minimum_headers)
        .expect("205 rows continue after a one-row page")
        .contains("limit=1&cursor="));
}

#[tokio::test]
async fn session_cursor_walk_has_no_overlap_and_status_is_an_exact_store_filter() {
    let TestSurface {
        app, store, caller, ..
    } = test_surface(true);
    seed_sessions(
        &store,
        &caller.subject,
        [
            SessionStatus::Running,
            SessionStatus::Ready,
            SessionStatus::Running,
            SessionStatus::Idle,
            SessionStatus::Running,
            SessionStatus::Terminated,
            SessionStatus::Running,
        ],
    )
    .await;

    let mut uri = "/api/v1/sessions?limit=2".to_string();
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        let (rows, headers) = get_collection(&app, &uri).await;
        pages += 1;
        assert!(pages <= 4, "seven rows at two per page cannot take longer");
        for row in rows {
            let id = row["id"].as_str().expect("session id").to_string();
            assert!(seen.insert(id.clone()), "cursor walk repeated {id}");
        }
        match next_link(&headers) {
            Some(next) => uri = next,
            None => break,
        }
    }
    assert_eq!(pages, 4);
    assert_eq!(seen.len(), 7);

    let (running, headers) = get_collection(&app, "/api/v1/sessions?status=running&limit=2").await;
    assert_eq!(running.len(), 2);
    assert!(running.iter().all(|row| row["status"] == "running"));
    let next = next_link(&headers).expect("four running rows need a second page");
    assert!(next.contains("status=running&limit=2&cursor="));
    let (running, headers) = get_collection(&app, &next).await;
    assert_eq!(running.len(), 2);
    assert!(running.iter().all(|row| row["status"] == "running"));
    assert!(next_link(&headers).is_none());
}

#[tokio::test]
async fn admin_sessions_and_environment_profiles_use_the_same_page_contract() {
    let TestSurface {
        app,
        store,
        profiles,
        caller,
        ..
    } = test_surface(true);
    seed_sessions(
        &store,
        &caller.subject,
        std::iter::repeat_n(SessionStatus::Ready, 3),
    )
    .await;
    seed_profiles(&profiles, &caller.subject, 3).await;

    for path in [
        "/api/v1/admin/sessions?limit=2",
        "/api/v1/environment-profiles?limit=2",
    ] {
        let (first, headers) = get_collection(&app, path).await;
        assert_eq!(first.len(), 2, "for {path}");
        let next = next_link(&headers).expect("the third row has a next page");
        let (last, headers) = get_collection(&app, &next).await;
        assert_eq!(last.len(), 1, "for {path}");
        assert!(next_link(&headers).is_none(), "for {path}");
    }
}

#[tokio::test]
async fn preferred_profile_response_pins_its_winner_across_bounded_pages() {
    let TestSurface {
        app,
        profiles,
        caller,
        ..
    } = test_surface(true);
    const PROFILE_COUNT: usize = 205;
    seed_profiles(&profiles, &caller.subject, PROFILE_COUNT).await;

    let pause = Arc::new(FirstProfilePagePause::new());
    *profiles.first_page_pause.lock().await = Some(Arc::clone(&pause));
    let request_app = app.clone();
    let pending = tokio::spawn(async move {
        assert_json(
            request_app
                .oneshot(json_request(
                    Method::PUT,
                    "/api/v1/environment-profiles/preferred",
                    r#"{"profile":"envp_page_0204"}"#,
                ))
                .await
                .expect("router answers"),
            StatusCode::OK,
        )
        .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), pause.reached.acquire())
        .await
        .expect("request A reaches its first profile page")
        .expect("the test still owns the page pause")
        .forget();
    assert_eq!(profiles.preference_writes.load(Ordering::SeqCst), 1);

    let second_page_winner = EnvironmentProfileId("envp_page_0000".into());
    profiles
        .set_preferred(&caller.subject, Some(&second_page_winner))
        .await
        .expect("a concurrent preference write succeeds");
    assert_eq!(
        profiles
            .preferred_for(&caller.subject)
            .await
            .expect("the concurrent winner is readable")
            .map(|profile| profile.id),
        Some(second_page_winner)
    );
    pause.resume.add_permits(1);

    let first = pending.await.expect("request A completes after it resumes");
    assert_complete_preference_response(&first, PROFILE_COUNT, Some("envp_page_0204"));
    assert_eq!(profiles.preference_writes.load(Ordering::SeqCst), 2);
    assert_eq!(profiles.list_calls.load(Ordering::SeqCst), 2);

    let cleared = assert_json(
        app.oneshot(json_request(
            Method::PUT,
            "/api/v1/environment-profiles/preferred",
            r#"{"profile":null}"#,
        ))
        .await
        .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_complete_preference_response(&cleared, PROFILE_COUNT, None);
    assert_eq!(profiles.preference_writes.load(Ordering::SeqCst), 3);
    assert_eq!(profiles.list_calls.load(Ordering::SeqCst), 4);
    let limits = profiles.list_limits.lock().await;
    assert_eq!(limits.len(), 4, "each response needs two store pages");
    assert!(
        limits.iter().all(|limit| *limit <= MAX_PAGE_SIZE),
        "no compatibility read may exceed the store page boundary: {limits:?}"
    );
    assert!(limits.iter().all(|limit| *limit == MAX_PAGE_SIZE));
    drop(limits);

    let stored = profiles.records.lock().await;
    let held: Vec<_> = stored
        .values()
        .filter(|profile| profile.preferred)
        .map(|profile| profile.id.0.clone())
        .collect();
    assert!(held.is_empty());
}

fn assert_complete_preference_response(
    body: &serde_json::Value,
    profile_count: usize,
    preferred_id: Option<&str>,
) {
    let rows = body.as_array().expect("the action response is an array");
    assert_eq!(rows.len(), profile_count);
    let actual_ids: Vec<_> = rows
        .iter()
        .map(|row| row["id"].as_str().expect("profile id").to_string())
        .collect();
    let expected_ids: Vec<_> = (0..profile_count)
        .rev()
        .map(|sequence| format!("envp_page_{sequence:04}"))
        .collect();
    assert_eq!(
        actual_ids, expected_ids,
        "the complete list stays newest first"
    );

    let preferred: Vec<_> = rows
        .iter()
        .filter(|row| row["preferred"].as_bool() == Some(true))
        .map(|row| row["id"].as_str().expect("profile id"))
        .collect();
    assert_eq!(preferred, preferred_id.into_iter().collect::<Vec<_>>());
}

#[tokio::test]
async fn every_session_response_uses_the_complete_projection() {
    let TestSurface {
        app,
        sessions,
        catalogue,
        caller,
        ..
    } = test_surface(true);

    let (id, created) = create_session(&app).await;
    assert_creator(&created);
    assert_named_reach(&created);
    assert!(
        created["agent"].get("skills").is_none(),
        "creation must not claim skills before provisioning records them: {created}"
    );
    assert_catalogue_received_credential(&catalogue, "create", false).await;
    wait_until_ready(&sessions, &caller, &id).await;

    let listed = assert_json(
        app.clone()
            .oneshot(
                Request::get("/api/v1/sessions")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["id"], id.0);
    assert_catalogue_received_credential(&catalogue, "list", true).await;

    let read = assert_json(
        app.clone()
            .oneshot(
                Request::get(format!("/api/v1/sessions/{}", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_catalogue_received_credential(&catalogue, "read", true).await;

    let changed_plane = assert_json(
        app.clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/api/v1/sessions/{}/plane", id.0),
                r#"{"plane":"conversation"}"#,
            ))
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    wait_until_ready(&sessions, &caller, &id).await;
    assert_catalogue_received_credential(&catalogue, "plane", true).await;

    let stopped = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/sessions/{}/stop", id.0))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(stopped.status(), StatusCode::NO_CONTENT);
    let revived = assert_json(
        app.clone()
            .oneshot(
                Request::post(format!("/api/v1/sessions/{}/revive", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_catalogue_received_credential(&catalogue, "revive", true).await;

    for view in [&listed[0], &read, &changed_plane, &revived] {
        assert_eq!(view["id"], id.0);
        assert_creator(view);
        assert_named_reach(view);
        assert_named_skills(view);
    }
}

#[tokio::test]
async fn directory_outages_cost_creator_names_not_session_responses() {
    let TestSurface {
        app,
        sessions,
        directory,
        caller,
        ..
    } = test_surface(true);
    directory.unavailable.store(true, Ordering::SeqCst);

    let (id, created) = create_session(&app).await;
    assert_creator_absent(&created);
    assert_named_reach(&created);
    assert!(created["agent"].get("skills").is_none());
    wait_until_ready(&sessions, &caller, &id).await;

    let listed = assert_json(
        app.clone()
            .oneshot(
                Request::get("/api/v1/sessions")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));

    let read = assert_json(
        app.clone()
            .oneshot(
                Request::get(format!("/api/v1/sessions/{}", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    let changed_plane = assert_json(
        app.clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/api/v1/sessions/{}/plane", id.0),
                r#"{"plane":"conversation"}"#,
            ))
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    wait_until_ready(&sessions, &caller, &id).await;

    let stopped = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/sessions/{}/stop", id.0))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(stopped.status(), StatusCode::NO_CONTENT);
    let revived = assert_json(
        app.clone()
            .oneshot(
                Request::post(format!("/api/v1/sessions/{}/revive", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;

    for view in [&listed[0], &read, &changed_plane, &revived] {
        assert_eq!(view["id"], id.0);
        assert_creator_absent(view);
        assert_named_reach(view);
        assert_named_skills(view);
    }
}

#[tokio::test]
async fn session_list_projects_catalogues_once_per_page() {
    let TestSurface {
        app,
        sessions,
        catalogue,
        caller,
        ..
    } = test_surface(true);
    create_ready_session(&app, &sessions, &caller).await;
    create_ready_session(&app, &sessions, &caller).await;
    catalogue.service_reads.store(0, Ordering::SeqCst);
    catalogue.skill_reads.store(0, Ordering::SeqCst);

    let listed = assert_json(
        app.clone()
            .oneshot(
                Request::get("/api/v1/sessions")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(2));
    for view in listed.as_array().expect("the response is a list") {
        assert_named_reach(view);
        assert_named_skills(view);
    }
    assert_eq!(catalogue.service_reads.load(Ordering::SeqCst), 1);
    assert_eq!(catalogue.skill_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn catalogue_outages_cost_names_not_session_responses() {
    let TestSurface {
        app,
        sessions,
        catalogue,
        caller,
        ..
    } = test_surface(true);
    catalogue.unavailable.store(true, Ordering::SeqCst);

    let (id, created) = create_session(&app).await;
    assert_creator(&created);
    assert_unnamed_reach(&created);
    assert!(created["agent"].get("skills").is_none());
    wait_until_ready(&sessions, &caller, &id).await;

    let listed = assert_json(
        app.clone()
            .oneshot(
                Request::get("/api/v1/sessions")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));

    let read = assert_json(
        app.clone()
            .oneshot(
                Request::get(format!("/api/v1/sessions/{}", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    let changed_plane = assert_json(
        app.clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/api/v1/sessions/{}/plane", id.0),
                r#"{"plane":"conversation"}"#,
            ))
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    wait_until_ready(&sessions, &caller, &id).await;

    let stopped = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/sessions/{}/stop", id.0))
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(stopped.status(), StatusCode::NO_CONTENT);
    let revived = assert_json(
        app.clone()
            .oneshot(
                Request::post(format!("/api/v1/sessions/{}/revive", id.0))
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;

    for view in [&listed[0], &read, &changed_plane, &revived] {
        assert_eq!(view["id"], id.0);
        assert_creator(view);
        assert_unnamed_reach(view);
        assert_unnamed_skills(view);
    }
}

#[tokio::test]
async fn a_domain_refusal_uses_the_same_problem_contract() {
    let response = test_surface(true)
        .app
        .oneshot(
            Request::get("/api/v1/sessions/sesn_missing")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::NOT_FOUND).await;
    assert_eq!(body["code"], "session_not_found");
}

#[tokio::test]
async fn the_production_body_capture_limit_returns_a_coded_413() {
    let response = test_surface(true)
        .app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/sessions",
            vec![b' '; OVERSIZED_BODY_LEN],
        ))
        .await
        .expect("router answers");
    assert_request_too_large(response).await;
}

#[tokio::test]
async fn unbuffered_json_and_optional_body_limits_return_the_same_coded_413() {
    let app = test_surface(true).app;
    for (uri, content_type) in [
        ("/api/v1/sessions/sesn_bad/input", Some("application/json")),
        (
            "/api/v1/sessions/sesn_bad/terminal-ticket",
            Some("application/json"),
        ),
        ("/api/v1/sessions/sesn_bad/terminal-ticket", None),
    ] {
        let mut request = Request::post(uri);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(vec![b' '; OVERSIZED_BODY_LEN]))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_request_too_large(response).await;
    }
}

#[tokio::test]
async fn rest_routing_failures_are_scoped_problem_documents() {
    let app = test_surface(true).app;

    for uri in ["/api/v1", "/api/v1/", "/api/v1/no-such-rest-route"] {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::NOT_FOUND).await;
        assert_eq!(body["code"], "not_found", "for {uri}");
    }

    let response = app
        .clone()
        .oneshot(
            Request::put("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let allow = response
        .headers()
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .expect("method refusals retain the Allow header");
    assert!(allow.split(',').any(|method| method == "GET"));
    assert!(allow.split(',').any(|method| method == "POST"));
    let body = assert_problem(response, StatusCode::METHOD_NOT_ALLOWED).await;
    assert_eq!(body["code"], "method_not_allowed");

    let response = app
        .oneshot(
            Request::get("/auth/not-a-browser-route")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_ne!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        )),
        "the REST fallback must remain scoped to /api/v1"
    );
}

#[tokio::test]
async fn well_formed_create_and_input_keep_their_success_statuses() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/sessions/{}/input", id.0),
            r#"{"text":"echo ready"}"#,
        ))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn one_create_key_returns_the_first_bytes_and_provisions_once() {
    let TestSurface {
        app, store, kernel, ..
    } = test_surface(true);

    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "create-retry-a",
            "{}",
        ))
        .await
        .expect("router answers");
    let first_status = first.status();
    let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();

    let replay = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "create-retry-a",
            "{}",
        ))
        .await
        .expect("router answers");
    let replay_status = replay.status();
    let replay_body = to_bytes(replay.into_body(), usize::MAX).await.unwrap();

    assert_eq!(first_status, StatusCode::CREATED);
    assert_eq!(replay_status, first_status);
    assert_eq!(
        replay_body, first_body,
        "the response body must replay verbatim"
    );
    assert_eq!(store.state.lock().await.records.len(), 1);
    for _ in 0..100 {
        if kernel.provisions.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(kernel.provisions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_input_key_delivers_one_prompt() {
    let TestSurface {
        app,
        sessions,
        caller,
        kernel,
        ..
    } = test_surface(true);
    let id = create_ready_conversation_session(&sessions, &caller).await;
    kernel.conversation_inputs.store(0, Ordering::SeqCst);
    let uri = format!("/api/v1/sessions/{}/input", id.0);

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(keyed_json_request(
                Method::POST,
                &uri,
                "prompt-retry-a",
                r#"{"text":"act once"}"#,
            ))
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
    }

    for _ in 0..100 {
        if kernel.conversation_inputs.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(kernel.conversation_inputs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn different_create_keys_create_two_sessions() {
    let TestSurface { app, store, .. } = test_surface(true);
    for key in ["create-a", "create-b"] {
        let response = app
            .clone()
            .oneshot(keyed_json_request(
                Method::POST,
                "/api/v1/sessions",
                key,
                "{}",
            ))
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::CREATED);
    }
    assert_eq!(store.state.lock().await.records.len(), 2);
}

#[tokio::test]
async fn malformed_operation_keys_are_named_bad_requests() {
    let app = test_surface(true).app;
    for key in ["", "contains space"] {
        let response = app
            .clone()
            .oneshot(keyed_json_request(
                Method::POST,
                "/api/v1/sessions",
                key,
                "{}",
            ))
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "invalid_idempotency_key");
    }
}

#[tokio::test]
async fn an_in_flight_duplicate_is_a_named_conflict() {
    use sha2::{Digest as _, Sha256};

    let TestSurface {
        app,
        operations,
        store,
        caller,
        ..
    } = test_surface(true);
    let fingerprint: [u8; 32] = Sha256::digest(b"{}").into();
    let claim = OperationClaim::new(
        caller.subject.clone(),
        "session-create",
        "still-running",
        fingerprint,
        SystemTime::now() + std::time::Duration::from_secs(86_400),
    );
    assert_eq!(
        operations.reserve(&claim).await.unwrap(),
        OperationReservation::Acquired
    );

    let response = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "still-running",
            "{}",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::CONFLICT).await;
    assert_eq!(body["code"], "idempotency_in_flight");
    assert!(store.state.lock().await.records.is_empty());
}

#[tokio::test]
async fn reusing_a_key_for_different_input_is_unprocessable() {
    let TestSurface { app, store, .. } = test_surface(true);
    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "changed-create",
            "{}",
        ))
        .await
        .expect("router answers");
    assert_eq!(first.status(), StatusCode::CREATED);

    let response = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "changed-create",
            r#"{"cols":100,"rows":40}"#,
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "idempotency_key_reused");
    assert_eq!(store.state.lock().await.records.len(), 1);
}

#[tokio::test]
async fn byte_different_bodies_do_not_replay_even_when_the_json_value_matches() {
    let TestSurface { app, store, .. } = test_surface(true);
    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "body-bytes",
            "{}",
        ))
        .await
        .expect("router answers");
    assert_eq!(first.status(), StatusCode::CREATED);

    let response = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "body-bytes",
            "{ }",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "idempotency_key_reused");
    assert_eq!(store.state.lock().await.records.len(), 1);
}

#[tokio::test]
async fn malformed_bytes_cannot_bypass_a_completed_keys_fingerprint() {
    let TestSurface { app, store, .. } = test_surface(true);
    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "malformed-retry",
            "{}",
        ))
        .await
        .expect("router answers");
    assert_eq!(first.status(), StatusCode::CREATED);

    let response = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "malformed-retry",
            "{",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "idempotency_key_reused");
    assert_eq!(store.state.lock().await.records.len(), 1);
}

#[tokio::test]
async fn a_keyed_body_refusal_replays_and_retains_its_raw_fingerprint() {
    let TestSurface {
        app,
        operations,
        store,
        ..
    } = test_surface(true);

    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "malformed-first",
            "{",
        ))
        .await
        .expect("router answers");
    let first_status = first.status();
    let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();

    let replay = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "malformed-first",
            "{",
        ))
        .await
        .expect("router answers");
    let replay_status = replay.status();
    let replay_body = to_bytes(replay.into_body(), usize::MAX).await.unwrap();
    assert_eq!(first_status, StatusCode::BAD_REQUEST);
    assert_eq!(replay_status, first_status);
    assert_eq!(replay_body, first_body);

    let mismatch = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "malformed-first",
            "[",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(mismatch, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "idempotency_key_reused");
    assert!(store.state.lock().await.records.is_empty());
    assert_eq!(operations.state.lock().await.len(), 1);
}

#[tokio::test]
async fn an_expired_key_is_reusable_and_a_stale_completion_cannot_replace_it() {
    use sha2::{Digest as _, Sha256};

    let operations = MemoryOperations::default();
    let fingerprint: [u8; 32] = Sha256::digest(b"{}").into();
    let expired = OperationClaim::new(
        SubjectId("subject-a".into()),
        "session-create",
        "expired",
        fingerprint,
        SystemTime::UNIX_EPOCH,
    );
    assert_eq!(
        operations.reserve(&expired).await.unwrap(),
        OperationReservation::Acquired
    );
    let current = OperationClaim::new(
        SubjectId("subject-a".into()),
        "session-create",
        "expired",
        fingerprint,
        SystemTime::now() + std::time::Duration::from_secs(86_400),
    );
    assert_eq!(
        operations.reserve(&current).await.unwrap(),
        OperationReservation::Acquired
    );
    assert!(operations
        .complete(&expired, b"stale".to_vec())
        .await
        .is_err());
    assert_eq!(
        operations.reserve(&current).await.unwrap(),
        OperationReservation::Pending
    );
}

#[tokio::test]
async fn an_unavailable_operation_store_fails_closed_before_creation() {
    let TestSurface {
        app,
        operations,
        store,
        kernel,
        ..
    } = test_surface(true);
    operations.unavailable.store(true, Ordering::SeqCst);

    let response = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "store-down",
            "{}",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(body["code"], "store_failed");
    assert!(store.state.lock().await.records.is_empty());
    assert_eq!(kernel.provisions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_failed_completion_keeps_the_claim_pending_and_prevents_a_second_create() {
    let TestSurface {
        app,
        operations,
        store,
        kernel,
        ..
    } = test_surface(true);
    operations
        .completions_unavailable
        .store(true, Ordering::SeqCst);

    let first = app
        .clone()
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "completion-down",
            "{}",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(first, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(body["code"], "store_failed");
    assert_eq!(store.state.lock().await.records.len(), 1);

    operations
        .completions_unavailable
        .store(false, Ordering::SeqCst);
    let retry = app
        .oneshot(keyed_json_request(
            Method::POST,
            "/api/v1/sessions",
            "completion-down",
            "{}",
        ))
        .await
        .expect("router answers");
    let body = assert_problem(retry, StatusCode::CONFLICT).await;
    assert_eq!(body["code"], "idempotency_in_flight");
    assert_eq!(store.state.lock().await.records.len(), 1);
    for _ in 0..100 {
        if kernel.provisions.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(kernel.provisions.load(Ordering::SeqCst), 1);
}

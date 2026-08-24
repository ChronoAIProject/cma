//! Production-composition checks for the REST error contract.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode},
    Extension, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer1_public::{auth::DelegatedCredential, tickets::TerminalTickets, AuthenticatedSubject};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, AgentConfiguration, AgentOptions,
    Authorization, Baseline, Caller, Capability, CatalogError, CatalogService, CatalogSkill,
    EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileStore,
    Event, KernelId, Page, PageCursor, PageRequest, PlatformSettingsService, RecoveryState,
    RegisteredUser, Reserved, ScreenFactory, ScreenModel, ScreenSnapshot, ServiceCatalog,
    ServiceId, Session, SessionId, SessionListQuery, SessionPorts, SessionRecord, SessionService,
    SessionStatus, SessionStore, SkillCatalog, SkillId, SkillPack, SkillSource, StoreError,
    StoredEvent, SubjectId, TerminalGeometry, UserDirectory, UserDirectoryStore, MAX_PAGE_SIZE,
};
use layer3_adapter::{
    AdapterError, KernelAdapter, KernelCapabilities, RuntimeHandle, RuntimeMode, RuntimeSpec,
    TerminalChannel,
};
use tokio::sync::{mpsc, Mutex};
use tower::ServiceExt;

use super::{rest_api_router, with_cors};
use crate::activity::{self, ActivityRecorder};

const OVERSIZED_BODY_LEN: usize = 2 * 1024 * 1024 + 1;
const LISTED_ORIGIN: &str = "https://integrator.example";
const UNLISTED_ORIGIN: &str = "https://unlisted.example";
const TEST_SERVICE_ID: &str = "api-github";
const TEST_SKILL_NAME: &str = "repository-review";
const TEST_DELEGATED_CREDENTIAL: &str = "contract-delegated-credential";

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

#[derive(Default)]
struct MemoryProfiles {
    records: Mutex<HashMap<String, EnvironmentProfile>>,
    list_calls: AtomicUsize,
    list_limits: Mutex<Vec<u32>>,
    preference_writes: AtomicUsize,
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
        Ok(page_profiles(
            self.records
                .lock()
                .await
                .values()
                .filter(|profile| &profile.owner == owner)
                .cloned()
                .collect(),
            page,
        ))
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

struct TestKernel;

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
            ..Default::default()
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
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
        _updates: mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> Result<layer3_adapter::ConversationChannel, AdapterError> {
        Ok(layer3_adapter::ConversationChannel {
            input: layer3_adapter::ConversationInputSender::spawn(8, |_input| async { Ok(()) }),
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
    store: Arc<MemorySessions>,
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
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("test"), Arc::new(TestKernel));
    let factory: ScreenFactory = Arc::new(|geometry| Box::new(EmptyScreen(geometry)));
    let store = Arc::new(MemorySessions::default());
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
    let app = rest_api_router(
        layer1_public::rest::SessionsState {
            sessions: Arc::clone(&sessions),
            users: Arc::clone(&users),
            tickets: TerminalTickets::new(),
            environments: Arc::clone(&environments),
        },
        Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&sessions),
            activity: Arc::clone(&activity),
            users,
            settings,
        }),
    )
    .layer(axum::middleware::from_fn_with_state(
        ActivityRecorder::new(activity),
        activity::record,
    ));
    let app = if authenticated {
        app.layer(Extension(caller.authorization.clone()))
            .layer(Extension(AuthenticatedSubject(caller.subject.clone())))
            .layer(Extension(DelegatedCredential(
                TEST_DELEGATED_CREDENTIAL.into(),
            )))
    } else {
        app
    };
    TestSurface {
        app,
        sessions,
        store,
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
    let value = headers.get(header::LINK)?.to_str().expect("Link is text");
    let target = value
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix(">; rel=\"next\""))
        .expect("Link has the RFC 8288 next relation shape");
    Some(target.to_string())
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
            "authorization,content-type",
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
            "/api/v1/sessions/session/events?after=17",
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
    let response = app
        .oneshot(preflight_request(
            "/api/v1/sessions",
            UNLISTED_ORIGIN,
            Method::POST,
            Some("authorization"),
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "an unlisted origin must not receive a browser grant"
    );
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
async fn preferred_profile_action_returns_every_profile_through_bounded_pages() {
    let TestSurface {
        app,
        profiles,
        caller,
        ..
    } = test_surface(true);
    const PROFILE_COUNT: usize = 205;
    seed_profiles(&profiles, &caller.subject, PROFILE_COUNT).await;

    let first = assert_json(
        app.clone()
            .oneshot(json_request(
                Method::PUT,
                "/api/v1/environment-profiles/preferred",
                r#"{"profile":"envp_page_0204"}"#,
            ))
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_complete_preference_response(&first, PROFILE_COUNT, "envp_page_0204");
    assert_eq!(profiles.preference_writes.load(Ordering::SeqCst), 1);
    assert_eq!(profiles.list_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        profiles.list_limits.lock().await.as_slice(),
        [MAX_PAGE_SIZE, MAX_PAGE_SIZE]
    );

    let moved = assert_json(
        app.clone()
            .oneshot(json_request(
                Method::PUT,
                "/api/v1/environment-profiles/preferred",
                r#"{"profile":"envp_page_0000"}"#,
            ))
            .await
            .expect("router answers"),
        StatusCode::OK,
    )
    .await;
    assert_complete_preference_response(&moved, PROFILE_COUNT, "envp_page_0000");
    assert_eq!(profiles.preference_writes.load(Ordering::SeqCst), 2);
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
    assert_eq!(held, vec!["envp_page_0000"]);
}

fn assert_complete_preference_response(
    body: &serde_json::Value,
    profile_count: usize,
    preferred_id: &str,
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
    assert_eq!(preferred, vec![preferred_id]);
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

//! Shared in-memory composition for Trigger route contract tests.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileSnapshot, AgentProfileStore,
    AgentProvider, InMemoryProfileStore, ModelAccess, PermissionLevel, ProfileCapacityBounds,
    ProfileVisibility, SandboxProfile, SandboxProfileContent, SandboxProfileRef,
    SandboxProfileStore, ThinkingEffort, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent, WorkspaceProfileRef, WorkspaceProfileStore,
};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
    FirstMessageRecord, FixedSandboxCredentialPrerequisite, Sandbox, SandboxCredentialPrerequisite,
    Workspace,
};
use layer2_core::trigger_domain::{
    BrokerHandoff, BrokerHandoffState, InMemoryLaunchStore, InMemoryTriggerStore,
    LaunchContextReader, LaunchResolutionError, LaunchService, ServiceConnectionBroker,
    ServiceConnectionError, ServiceConnectionMetadata, ServiceConnectionService,
    ServiceRequirement, ServiceRequirementState, TriggerProfileResolver, TriggerService,
};
use layer2_core::{
    rfc3339_at, AgentId, AgentProfileId, Authorization, Capability, DirectoryQuery, OperationClaim,
    OperationReservation, OperationStore, Page, PublicIdentity, RuntimeCapabilities,
    RuntimeCapability, SandboxId, SandboxProfileId, ServiceId, StoreError, SubjectId,
    UserDirectory, UserDirectoryStore, WorkspaceId, WorkspaceProfileId, WorktreeId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::super::{
    trigger_assets_router, trigger_router, LaunchQuota, LaunchQuotaError, LaunchQuotaPermit,
    LaunchQuotaRefusal, TriggerAssetsState, TriggerRoutesState,
};
use super::store_faults::{FaultLaunchStore, FaultPolicy, FaultTriggerStore};
use crate::AuthenticatedSubject;

pub const OWNER: &str = "publisher";
pub const VISITOR: &str = "visitor";
pub const OTHER: &str = "other";
const NOW: &str = "2026-09-05T00:00:00Z";

type OperationKey = (String, String, String);
type OperationRecord = ([u8; 32], Option<Vec<u8>>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompletionFault {
    BeforeWrite,
    AfterWrite,
}

#[derive(Default)]
struct MemoryOperationStore {
    records: Mutex<HashMap<OperationKey, OperationRecord>>,
    reservations: AtomicUsize,
    failure: Mutex<Option<Result<OperationReservation, StoreError>>>,
    completion_fault: Mutex<Option<CompletionFault>>,
}

#[async_trait]
impl OperationStore for MemoryOperationStore {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        self.reservations.fetch_add(1, Ordering::SeqCst);
        if let Some(result) = self.failure.lock().unwrap().as_ref() {
            return result.clone();
        }
        let key = (
            claim.subject().to_string(),
            claim.operation().to_owned(),
            claim.key().to_owned(),
        );
        let mut records = self.records.lock().unwrap();
        match records.get(&key) {
            Some((fingerprint, _)) if fingerprint != claim.fingerprint() => {
                Ok(OperationReservation::FingerprintMismatch)
            }
            Some((_, Some(outcome))) => Ok(OperationReservation::Complete(outcome.clone())),
            Some((_, None)) => Ok(OperationReservation::Pending),
            None => {
                records.insert(key, (*claim.fingerprint(), None));
                Ok(OperationReservation::Acquired)
            }
        }
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let fault = *self.completion_fault.lock().unwrap();
        if fault == Some(CompletionFault::BeforeWrite) {
            return Err(StoreError::Unavailable(
                "synthetic receipt write failure".into(),
            ));
        }
        let key = (
            claim.subject().to_string(),
            claim.operation().to_owned(),
            claim.key().to_owned(),
        );
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&key)
            .ok_or_else(|| StoreError::Malformed("operation claim disappeared".into()))?;
        record.1 = Some(outcome);
        if fault == Some(CompletionFault::AfterWrite) {
            return Err(StoreError::Unavailable(
                "synthetic receipt acknowledgement loss".into(),
            ));
        }
        Ok(())
    }
}

struct EmptyDirectory;

#[async_trait]
impl UserDirectoryStore for EmptyDirectory {
    async fn observe(&self, _identity: &PublicIdentity) -> Result<(), StoreError> {
        Ok(())
    }

    async fn user(
        &self,
        _subject: &SubjectId,
    ) -> Result<Option<layer2_core::RegisteredUser>, StoreError> {
        Ok(None)
    }

    async fn list(
        &self,
        _query: &DirectoryQuery,
    ) -> Result<Page<layer2_core::RegisteredUser>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

#[derive(Clone, Default)]
pub struct StubContext {
    agent: Arc<Mutex<Option<Agent>>>,
    sandbox: Arc<Mutex<Option<Sandbox>>>,
}

impl StubContext {
    pub fn set_agent(&self, agent: Agent) {
        *self.agent.lock().unwrap() = Some(agent);
    }
}

#[async_trait]
impl LaunchContextReader for StubContext {
    async fn owned_sandbox(
        &self,
        owner: &SubjectId,
        id: &SandboxId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        Ok(self
            .sandbox
            .lock()
            .unwrap()
            .clone()
            .filter(|sandbox| sandbox.owner == *owner && sandbox.id == *id))
    }
    async fn newest_matching_sandbox(
        &self,
        _owner: &SubjectId,
        _profile_id: &SandboxProfileId,
    ) -> Result<Option<Sandbox>, LaunchResolutionError> {
        Ok(None)
    }

    async fn matching_workspace(
        &self,
        _owner: &SubjectId,
        _sandbox_id: &SandboxId,
        _profile_id: &WorkspaceProfileId,
    ) -> Result<Option<Workspace>, LaunchResolutionError> {
        Ok(None)
    }

    async fn newest_live_trigger_agent(
        &self,
        _owner: &SubjectId,
        _trigger_id: &layer2_core::trigger_domain::TriggerId,
    ) -> Result<Option<Agent>, LaunchResolutionError> {
        Ok(self.agent.lock().unwrap().clone())
    }
}

struct BrokerState {
    requirement: ServiceRequirementState,
    create_calls: usize,
    observed: BrokerHandoffState,
    metadata: Vec<ServiceConnectionMetadata>,
    metadata_reads: usize,
    failure: Option<ServiceConnectionError>,
}

impl Default for BrokerState {
    fn default() -> Self {
        Self {
            requirement: ServiceRequirementState::Missing,
            create_calls: 0,
            observed: BrokerHandoffState::Pending,
            metadata: Vec::new(),
            metadata_reads: 0,
            failure: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct StubBroker {
    state: Arc<Mutex<BrokerState>>,
}

impl StubBroker {
    pub fn fail(&self, error: ServiceConnectionError) {
        self.state.lock().unwrap().failure = Some(error);
    }

    fn check(&self) -> Result<(), ServiceConnectionError> {
        match self.state.lock().unwrap().failure.as_ref() {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
    pub fn set_metadata(&self, metadata: Vec<ServiceConnectionMetadata>) {
        self.state.lock().unwrap().metadata = metadata;
    }

    pub fn metadata_reads(&self) -> usize {
        self.state.lock().unwrap().metadata_reads
    }

    pub fn set_requirement(&self, state: ServiceRequirementState) {
        self.state.lock().unwrap().requirement = state;
    }

    pub fn observe(&self, state: BrokerHandoffState) {
        self.state.lock().unwrap().observed = state;
    }

    pub fn create_calls(&self) -> usize {
        self.state.lock().unwrap().create_calls
    }
}

#[async_trait]
impl ServiceConnectionBroker for StubBroker {
    async fn describe_services(
        &self,
        _owner: &SubjectId,
        _services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        self.check()?;
        let mut state = self.state.lock().unwrap();
        state.metadata_reads += 1;
        Ok(state.metadata.clone())
    }

    async fn requirements(
        &self,
        _owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        self.check()?;
        let state = self.state.lock().unwrap().requirement;
        Ok(services
            .iter()
            .cloned()
            .map(|service| ServiceRequirement { service, state })
            .collect())
    }

    async fn create_handoff(
        &self,
        _owner: &SubjectId,
        _service: &ServiceId,
        intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        self.check()?;
        self.state.lock().unwrap().create_calls += 1;
        Ok(BrokerHandoff {
            external_id: format!("external-{intent_id}"),
            hosted_url: format!("https://identity.example/connect/{intent_id}"),
            expires_at: rfc3339_at(SystemTime::now() + Duration::from_secs(900)),
        })
    }

    async fn handoff_state(
        &self,
        _owner: &SubjectId,
        _external_id: &str,
        _service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        self.check()?;
        Ok(self.state.lock().unwrap().observed)
    }
}

#[derive(Default)]
struct QuotaState {
    reserve_calls: AtomicUsize,
    settlements: Mutex<Vec<bool>>,
    refuse: AtomicBool,
    unavailable: AtomicBool,
}

#[derive(Clone, Default)]
pub struct TestQuota {
    state: Arc<QuotaState>,
}

impl TestQuota {
    pub fn unavailable(&self) {
        self.state.unavailable.store(true, Ordering::SeqCst);
    }
    pub fn refuse(&self) {
        self.state.refuse.store(true, Ordering::SeqCst);
    }

    pub fn reserve_calls(&self) -> usize {
        self.state.reserve_calls.load(Ordering::SeqCst)
    }

    pub fn settlements(&self) -> Vec<bool> {
        self.state.settlements.lock().unwrap().clone()
    }
}

#[async_trait]
impl LaunchQuota for TestQuota {
    async fn reserve(
        &self,
        _caller: &layer2_core::Caller,
    ) -> Result<LaunchQuotaPermit, LaunchQuotaError> {
        self.state.reserve_calls.fetch_add(1, Ordering::SeqCst);
        if self.state.unavailable.load(Ordering::SeqCst) {
            return Err(LaunchQuotaError::Unavailable);
        }
        if self.state.refuse.load(Ordering::SeqCst) {
            return Err(LaunchQuotaError::ResourceQuota(
                LaunchQuotaRefusal::visitor_launches(
                    5,
                    SystemTime::now() + Duration::from_secs(60),
                ),
            ));
        }
        let state = Arc::clone(&self.state);
        Ok(LaunchQuotaPermit::reserved(move |created| {
            state.settlements.lock().unwrap().push(created);
        }))
    }
}

pub struct TestApp {
    pub source_service: Arc<TriggerService>,
    pub router: Router,
    pub contexts: Arc<StubContext>,
    pub broker: Arc<StubBroker>,
    pub quota: Arc<TestQuota>,
    pub definitions: Arc<FaultTriggerStore>,
    pub launches: Arc<FaultLaunchStore>,
    pub policy: Arc<FaultPolicy>,
    operations: Arc<MemoryOperationStore>,
    pub agent_snapshot: AgentProfileSnapshot,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::configured(
            false,
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            None,
            Default::default(),
        )
        .await
    }

    pub async fn private_profiles() -> Self {
        Self::configured(
            true,
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            None,
            Default::default(),
        )
        .await
    }

    pub async fn with_credential_prerequisite(
        prerequisite: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Self {
        Self::configured(false, prerequisite, None, Default::default()).await
    }

    pub async fn with_inline_materializer(
        materializer: Arc<super::inline_image_routes::CredentialMaterializer>,
    ) -> Self {
        Self::configured(
            false,
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            Some(materializer),
            Default::default(),
        )
        .await
    }

    pub async fn with_admission_policy(
        policy: layer2_core::trigger_domain::LaunchAdmissionPolicy,
        prerequisite: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Self {
        Self::configured(false, prerequisite, None, policy).await
    }

    async fn configured(
        private_profiles: bool,
        prerequisite: Arc<dyn SandboxCredentialPrerequisite>,
        materializer: Option<Arc<super::inline_image_routes::CredentialMaterializer>>,
        admission_policy: layer2_core::trigger_domain::LaunchAdmissionPolicy,
    ) -> Self {
        let profiles = Arc::new(InMemoryProfileStore::default());
        let (agent, _workspace, sandbox) = publish_profiles(&profiles, private_profiles).await;
        let resolver: Arc<dyn TriggerProfileResolver> = profiles.clone();
        let memory_definitions = Arc::new(InMemoryTriggerStore::new(resolver));
        let launches = Arc::new(FaultLaunchStore::new(Arc::new(InMemoryLaunchStore::new(
            memory_definitions.clone(),
        ))));
        let definitions = Arc::new(FaultTriggerStore::new(memory_definitions));
        let policy = Arc::new(FaultPolicy::default());
        let contexts = Arc::new(StubContext::default());
        let revision = profiles
            .sandbox_profile_revision(&sandbox.id, sandbox.revision)
            .await
            .unwrap()
            .unwrap();
        {
            use layer2_core::sandbox::{
                SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
                SandboxStatus, SandboxWorkState, SandboxWorkStep,
            };
            *contexts.sandbox.lock().unwrap() = Some(Sandbox {
                id: SandboxId("sbx_existing".into()),
                owner: SubjectId::new(VISITOR),
                name: "Existing Sandbox".into(),
                origin: SandboxOrigin::Manual,
                profile: layer2_core::profiles::SandboxProfileSnapshot::from(&revision),
                status: SandboxStatus::Active,
                failure: None,
                runtime: None,
                credential: SandboxCredentialState {
                    source: Default::default(),
                    reaches: revision.content.credential_services(&[]),
                    service_grants: Vec::new(),
                    service_scope: None,
                    key_generation: 0,
                    delivered: false,
                    current: None,
                    pending: None,
                    revoke_debt: Vec::new(),
                    sign_in: SandboxSignInState::NotRequired,
                },
                work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
                limits: SandboxLimits {
                    max_workspaces: 4,
                    max_agents_per_workspace: 4,
                },
                lifecycle: Default::default(),
                last_activity_at: NOW.into(),
                created_at: NOW.into(),
                updated_at: NOW.into(),
                retired_at: None,
                purge_after: None,
                revision: 1,
            });
        }
        let broker = Arc::new(StubBroker::default());
        broker.set_requirement(ServiceRequirementState::Missing);
        let directory = UserDirectory::new(Arc::new(EmptyDirectory));
        let trigger_service = match materializer {
            Some(materializer) => TriggerService::new_with_inline_materializer(
                definitions.clone(),
                launches.clone(),
                directory,
                materializer,
            ),
            None => TriggerService::new(definitions.clone(), launches.clone(), directory),
        };
        let launch_service = LaunchService::new(
            definitions.clone(),
            profiles.clone(),
            launches.clone(),
            contexts.clone(),
            broker.clone(),
            policy.clone(),
            prerequisite,
        )
        .with_admission_policy(admission_policy);
        let connections = ServiceConnectionService::new(launches.clone(), broker.clone());
        let operations = Arc::new(MemoryOperationStore::default());
        let operation_store: Arc<dyn OperationStore> = operations.clone();
        let quota = Arc::new(TestQuota::default());
        let launch_quota: Arc<dyn LaunchQuota> = quota.clone();
        let router = trigger_router(TriggerRoutesState::new(
            trigger_service.clone(),
            launch_service,
            connections,
            operation_store,
            launch_quota,
        ))
        .merge(trigger_assets_router(
            TriggerAssetsState::new(definitions.clone(), "https://cma.example")
                .expect("fixture public origin is valid"),
        ))
        .merge(crate::agui::launch_companion_router(
            crate::agui::LaunchCompanionState::new(launches.clone()),
        ));
        Self {
            source_service: trigger_service,
            router,
            contexts,
            broker,
            quota,
            definitions,
            launches,
            policy,
            operations,
            agent_snapshot: AgentProfileSnapshot::from(&agent),
        }
    }

    pub fn operation_reservations(&self) -> usize {
        self.operations.reservations.load(Ordering::SeqCst)
    }

    pub fn fail_completion(&self, fault: Option<CompletionFault>) {
        *self.operations.completion_fault.lock().unwrap() = fault;
    }

    pub fn fail_operations(&self, failure: Result<OperationReservation, StoreError>) {
        *self.operations.failure.lock().unwrap() = Some(failure);
    }

    pub fn set_existing_agent(&self, trigger_id: &str) {
        self.contexts.set_agent(Agent {
            id: AgentId("agt_existing".into()),
            worktree_id: WorktreeId("wkt_existing".into()),
            workspace_id: WorkspaceId("wks_existing".into()),
            sandbox_id: SandboxId("sbx_existing".into()),
            owner: SubjectId::new(VISITOR),
            creation_key: "trigger-key".into(),
            creation_fingerprint: "a".repeat(64),
            profile: self.agent_snapshot.clone(),
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            status: AgentStatus::Ready,
            conversation: ConversationState {
                turn_timeout: None,
                preferences: Default::default(),
                managed_guidance_revision: 0,
                observations: Default::default(),
                settings: None,
                conversation_id: Some("conversation".into()),
                runtime_generation: Some(1),
                last_seq: 1,
                first_message: Some(
                    FirstMessageRecord::new("existing", "e".repeat(180), NOW).unwrap(),
                ),
                active_turn: None,
                operation: None,
                queued_messages: Vec::new(),
            },
            work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
            failure: None,
            title: Some("Existing Agent".into()),
            created_at: NOW.into(),
            updated_at: NOW.into(),
            ready_at: Some(NOW.into()),
            stopped_at: None,
            requested_stop_reason: None,
            stop_reason: None,
            deleted_at: None,
            origin: AgentOrigin::Trigger {
                trigger_id: trigger_id.into(),
                launch_id: "lch_existing".into(),
            },
            revision: 1,
        });
    }
}

pub fn publisher_authority() -> Authorization {
    Authorization::new([Capability::PublishTriggers])
}

pub fn visitor_authority() -> Authorization {
    Authorization::new([Capability::LaunchTriggers])
}

pub fn request(
    method: Method,
    path: &str,
    body: Option<Value>,
    subject: Option<(&str, Authorization)>,
    key: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    if let Some((subject, authorization)) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(subject)));
        request.extensions_mut().insert(authorization);
    }
    request
}

pub fn publisher_request(
    method: Method,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> Request<Body> {
    request(
        method,
        path,
        body,
        Some((OWNER, publisher_authority())),
        key,
    )
}

pub fn visitor_request(
    method: Method,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> Request<Body> {
    request(
        method,
        path,
        body,
        Some((VISITOR, visitor_authority())),
        key,
    )
}

pub async fn raw_response(
    router: &Router,
    request: Request<Body>,
) -> (StatusCode, HeaderMap, Bytes) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, body)
}

pub async fn json_response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = raw_response(router, request).await;
    (status, serde_json::from_slice(&body).unwrap())
}

pub fn trigger_body(agent_reuse: &str) -> Value {
    json!({
        "content": {
            "presentation": {
                "title": "Launch a proof Agent",
                "description": "A public Trigger used by route tests.",
                "button_label": "open_in_cma",
                "theme": "auto",
                "signup_hint": "Use your existing NyxID account"
            },
            "target": {
                "sandbox_profile": { "id": "sbp_route_sandbox", "revision": 1 },
                "workspace_profile": { "id": "wkp_route_workspace", "revision": 1 },
                "agent_profile": { "id": "agp_route_agent", "revision": 1 }
            },
            "reuse": {
                "sandbox": "always_new",
                "workspace": "always_new",
                "agent": agent_reuse
            },
            "first_message": "x".repeat(180),
            "consent": { "always_review": false }
        }
    })
}

pub async fn create_trigger(app: &TestApp, agent_reuse: &str, key: &str) -> Value {
    let (status, body) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            "/api/v1/triggers",
            Some(trigger_body(agent_reuse)),
            Some(key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

pub async fn publish_trigger(app: &TestApp, trigger_id: &str, key: &str) -> Value {
    let (status, body) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{trigger_id}/publish"),
            Some(json!({ "draft_version": 1 })),
            Some(key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

pub fn launch_body(start_if_ready: bool) -> Value {
    json!({
        "placement_id": null,
        "referrer_origin": "https://source.example",
        "start_if_ready": start_if_ready,
        "choice": null
    })
}

async fn publish_profiles(
    profiles: &InMemoryProfileStore,
    private: bool,
) -> (
    layer2_core::profiles::AgentProfileRevision,
    WorkspaceProfileRef,
    SandboxProfileRef,
) {
    let agent_id = AgentProfileId("agp_route_agent".into());
    profiles
        .create_agent_profile(AgentProfile {
            internal: false,
            id: agent_id.clone(),
            owner: SubjectId::new(OWNER),
            name: "Route Agent".into(),
            description: String::new(),
            visibility: if private {
                ProfileVisibility::Private
            } else {
                ProfileVisibility::Public
            },
            forked_from: None,
            uses_count: 0,
            draft: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: Vec::new(),
                system_prompt: String::new(),
            },
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let agent = profiles
        .publish_agent_profile(&agent_id, 1, NOW.into())
        .await
        .unwrap();
    let agent_ref = AgentProfileRef::new(agent_id, 1);

    let workspace_id = WorkspaceProfileId("wkp_route_workspace".into());
    profiles
        .create_workspace_profile(WorkspaceProfile {
            internal: false,
            id: workspace_id.clone(),
            owner: SubjectId::new(OWNER),
            name: "Route Workspace".into(),
            description: String::new(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "route-project".into(),
                },
                name: "route-project".into(),
                setup_commands: Vec::new(),
                agent_profiles: vec![agent_ref],
            },
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    profiles
        .publish_workspace_profile(&workspace_id, 1, NOW.into())
        .await
        .unwrap();
    let workspace_ref = WorkspaceProfileRef::new(workspace_id, 1);

    let sandbox_id = SandboxProfileId("sbp_route_sandbox".into());
    profiles
        .create_sandbox_profile(SandboxProfile {
            internal: false,
            availability: Default::default(),
            id: sandbox_id.clone(),
            owner: SubjectId::new(OWNER),
            name: "Route Sandbox".into(),
            description: String::new(),
            visibility: ProfileVisibility::Public,
            forked_from: None,
            uses_count: 0,
            draft: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![ServiceId::new("github")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![workspace_ref.clone()],
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    profiles
        .publish_sandbox_profile(
            &sandbox_id,
            1,
            NOW.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .unwrap();
    (agent, workspace_ref, SandboxProfileRef::new(sandbox_id, 1))
}

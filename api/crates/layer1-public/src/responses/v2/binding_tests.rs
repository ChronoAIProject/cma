use super::tests::context;
use crate::rest::agents_tests as fixtures;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::*;
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::*,
    sandbox::{
        agent_io::*,
        store::{AgentStore, InMemorySandboxStore, PageRequest, SandboxStore},
        AgentListQuery, AgentStatus, OwnerSandboxQuery,
    },
    Caller, SubjectId,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;

struct BindingAuthority {
    bindings: Arc<BindingAccessService>,
    admission_race: Option<Arc<InMemorySandboxStore>>,
    changed: AtomicBool,
}
#[async_trait]
impl AgentCommandAdmissionAuthority for BindingAuthority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        _: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let principal = self
            .bindings
            .authenticate(&context.credential.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner, principal.resource_owner());
        if let Some(store) = &self.admission_race {
            if !self.changed.swap(true, Ordering::SeqCst) {
                let mut agent = store
                    .get_agent(&layer2_core::AgentId("agt_fixture".into()))
                    .await
                    .unwrap()
                    .unwrap();
                let revision = agent.revision;
                agent.revision += 1;
                store.save_agent(revision, agent).await.unwrap();
            }
        }
        Ok(principal.command_authority())
    }
}
#[async_trait]
impl OrganizationAuthority for BindingAuthority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("binding alias must stay local")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("binding alias must stay local")
    }
}
fn binding(id: &str, hash: &str) -> ProviderBinding {
    binding_with_owner(id, hash, "human")
}

fn binding_with_owner(id: &str, hash: &str, owner_kind: &str) -> ProviderBinding {
    let value = json!({"schema_version":1,
        "metadata":{"name":"responses", "owner":{"kind":owner_kind,"subject":"owner-a"},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let plan = map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "responses",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    ProviderBinding::new(
        id.into(),
        SubjectId::new("manager"),
        plan,
        hash.into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap()
}
async fn ready(store: &InMemorySandboxStore, mut binding: ProviderBinding) -> ProviderBinding {
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    binding
}
pub(crate) fn request(path: &str, principal: &BindingPrincipal, body: Value) -> Request<Body> {
    let mut request = fixtures::request(
        Method::POST,
        path,
        principal.owner().as_str(),
        Some("binding-turn"),
        Some(body),
    );
    let mut context = context(principal.owner().as_str());
    context.credential_source = CredentialSource::Binding;
    context.caller = principal.caller();
    context.credential.issuer = "binding:local".into();
    context.credential.credential_id = principal.key_sha256().into();
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(principal.caller().authorization);
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("local-fixture".into()));
    request
}

struct UnavailableContextCommands {
    inner: Arc<dyn AgentCommandStore>,
}

#[async_trait]
impl AgentCommandStore for UnavailableContextCommands {
    async fn get_external_event_command(
        &self,
        _: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        Err(CommandLedgerError::Unavailable)
    }

    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }

    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

struct ArmOnExternalLookup {
    inner: Arc<dyn AgentCommandStore>,
    phase: Arc<AtomicUsize>,
}

struct RetireAfterExternalLookup {
    inner: Arc<InMemorySandboxStore>,
    binding_id: String,
}

async fn retire_binding(store: &InMemorySandboxStore, binding_id: &str) {
    let mut binding = store.binding(binding_id).await.unwrap().unwrap();
    binding.state = BindingState::Retired;
    binding.work = BindingWork::RetireCurrentConversations;
    let expected = binding.revision;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(expected), binding).await.unwrap();
}

#[async_trait]
impl AgentCommandStore for RetireAfterExternalLookup {
    async fn get_external_event_command(
        &self,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let result = self.inner.get_external_event_command(key).await;
        retire_binding(&self.inner, &self.binding_id).await;
        result
    }

    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }

    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

#[async_trait]
impl AgentCommandStore for ArmOnExternalLookup {
    async fn get_external_event_command(
        &self,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let result = self.inner.get_external_event_command(key).await;
        self.phase.store(1, Ordering::SeqCst);
        result
    }

    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.inner.reserve_command(request).await
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }

    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

struct RetireOnFenceStore {
    inner: Arc<InMemorySandboxStore>,
    phase: Arc<AtomicUsize>,
}

#[async_trait]
impl ResourceAccessStore for RetireOnFenceStore {
    async fn organization_sandboxes(
        &self,
        organization: &SubjectId,
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(organization, query).await
    }

    async fn shared_sandboxes(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(recipient, organizations, query)
            .await
    }

    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .organization_agents(organization, query, page)
            .await
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        if self
            .phase
            .compare_exchange(2, 3, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(ResourceAccessStoreError::Unavailable);
        } else {
            let _ = self
                .phase
                .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
        }
        self.inner.resource_access_snapshot(target).await
    }
}

#[async_trait]
impl ResourceShareStore for RetireOnFenceStore {
    async fn share_mutation_receipt(
        &self,
        operation_id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(operation_id).await
    }

    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.inner.mutate_share(request).await
    }

    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
}

pub(crate) async fn fixture() -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_with_conversation_state(BindingConversationState::Ready).await
}

async fn fixture_with_conversation_state(
    conversation_state: BindingConversationState,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_with_state_and_race(conversation_state, false).await
}

pub(crate) async fn fixture_with_admission_race() -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_with_state_and_race(BindingConversationState::Ready, true).await
}

async fn fixture_with_state_and_race(
    conversation_state: BindingConversationState,
    admission_race: bool,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner(conversation_state, admission_race, "human").await
}

pub(crate) async fn fixture_for_owner(
    conversation_state: BindingConversationState,
    admission_race: bool,
    owner_kind: &str,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_router(conversation_state, admission_race, owner_kind, None).await
}

pub(crate) async fn fixture_for_owner_with_router(
    conversation_state: BindingConversationState,
    admission_race: bool,
    owner_kind: &str,
    router: Option<Arc<dyn super::BindingEventRouter>>,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_router_and_context(
        conversation_state,
        admission_race,
        owner_kind,
        router,
        false,
    )
    .await
}

pub(crate) async fn fixture_for_owner_with_router_and_context(
    conversation_state: BindingConversationState,
    admission_race: bool,
    owner_kind: &str,
    router: Option<Arc<dyn super::BindingEventRouter>>,
    retained_context_lookup: bool,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_context_mode(ContextFixtureMode {
        conversation_state,
        admission_race,
        owner_kind,
        router,
        retained_context_lookup,
        unavailable_context: false,
        fence_race: false,
        revoke_on_lookup: false,
    })
    .await
}

pub(crate) async fn fixture_for_owner_with_unavailable_context(
    conversation_state: BindingConversationState,
    owner_kind: &str,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_context_mode(ContextFixtureMode {
        conversation_state,
        admission_race: false,
        owner_kind,
        router: None,
        retained_context_lookup: true,
        unavailable_context: true,
        fence_race: false,
        revoke_on_lookup: false,
    })
    .await
}

pub(crate) async fn fixture_for_owner_with_fence_race(
    conversation_state: BindingConversationState,
    owner_kind: &str,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_context_mode(ContextFixtureMode {
        conversation_state,
        admission_race: false,
        owner_kind,
        router: None,
        retained_context_lookup: true,
        unavailable_context: false,
        fence_race: true,
        revoke_on_lookup: false,
    })
    .await
}

pub(crate) async fn fixture_for_owner_with_revocation_race(
    conversation_state: BindingConversationState,
    owner_kind: &str,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    fixture_for_owner_with_context_mode(ContextFixtureMode {
        conversation_state,
        admission_race: false,
        owner_kind,
        router: None,
        retained_context_lookup: true,
        unavailable_context: false,
        fence_race: false,
        revoke_on_lookup: true,
    })
    .await
}

struct ContextFixtureMode<'a> {
    conversation_state: BindingConversationState,
    admission_race: bool,
    owner_kind: &'a str,
    router: Option<Arc<dyn super::BindingEventRouter>>,
    retained_context_lookup: bool,
    unavailable_context: bool,
    fence_race: bool,
    revoke_on_lookup: bool,
}

async fn fixture_for_owner_with_context_mode<'a>(
    mode: ContextFixtureMode<'a>,
) -> (
    axum::Router,
    Arc<InMemorySandboxStore>,
    Arc<BindingAccessService>,
    BindingPrincipal,
) {
    let (state, store) = fixtures::conversation_fixture().await;
    let agent = fixtures::seeded_agent("agt_fixture", AgentStatus::Ready, 2, None);
    fixtures::insert_agent(&store, agent.clone(), vec![]).await;
    let mut reserved = binding_with_owner("binding-responses", &"a".repeat(64), mode.owner_kind);
    reserved.sandbox_id = agent.sandbox_id.clone();
    reserved.workspace_id = agent.workspace_id.clone();
    let binding = ready(&store, reserved).await;
    let sandbox = store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    if let Some(policy) = binding.sandbox_resource_policy(&sandbox).unwrap() {
        store.set_resource_policy_for_test(policy);
    }
    let mut conversation = BindingConversation::new(
        &binding,
        "conversation-external".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    conversation.agent_id = agent.id.clone();
    conversation.worktree_id = agent.worktree_id.clone();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation.state = mode.conversation_state;
    conversation.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(1), conversation)
        .await
        .unwrap();
    let bindings = BindingAccessService::new(store.clone());
    let principal = bindings
        .authenticate(&binding.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let authority = Arc::new(BindingAuthority {
        bindings: bindings.clone(),
        admission_race: mode.admission_race.then(|| store.clone()),
        changed: AtomicBool::new(false),
    });
    let phase = Arc::new(AtomicUsize::new(0));
    let resource_store: Arc<dyn ResourceShareStore> = if mode.fence_race {
        Arc::new(RetireOnFenceStore {
            inner: store.clone(),
            phase: phase.clone(),
        })
    } else {
        store.clone()
    };
    let access = Arc::new(
        ResourceAccessService::new(resource_store, authority.clone())
            .with_bindings(bindings.clone()),
    );
    let command_store: Arc<dyn AgentCommandStore> = if mode.unavailable_context {
        Arc::new(UnavailableContextCommands {
            inner: store.clone(),
        })
    } else if mode.revoke_on_lookup {
        Arc::new(RetireAfterExternalLookup {
            inner: store.clone(),
            binding_id: binding.binding_id.clone(),
        })
    } else if mode.fence_race {
        Arc::new(ArmOnExternalLookup {
            inner: store.clone(),
            phase,
        })
    } else {
        store.clone()
    };
    let commands = AgentCommandService::new(command_store, access.clone(), authority);
    let mut state = super::ResponsesV2State::new(
        crate::agent_page::AgentPageOrigin::test(),
        commands,
        state.reads.with_resource_access(access),
    )
    .with_bindings(bindings.clone())
    .with_retained_context_lookup(mode.retained_context_lookup);
    if let Some(router) = mode.router {
        state = state.with_event_router(router);
    }
    let app = super::router(state);
    (app, store, bindings, principal)
}

#[tokio::test]
async fn provisioning_conversation_is_retryable_then_admits_the_same_turn_once() {
    let (app, store, _, principal) =
        fixture_with_conversation_state(BindingConversationState::Provisioning).await;
    let body = json!({"conversation":"conversation-external","stream":true,"input":"First event"});
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "6");
    assert!(response.headers().get("x-cma-response-id").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let problem: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem["error"]["code"], "conversation_not_ready");

    let binding = store
        .binding(principal.binding_id())
        .await
        .unwrap()
        .unwrap();
    let mut conversation = store
        .binding_conversation(
            principal.binding_id(),
            principal.generation(),
            "conversation-external",
        )
        .await
        .unwrap()
        .unwrap();
    let prior_revision = conversation.revision;
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(prior_revision), conversation)
        .await
        .unwrap();
    let mut response_id = None;
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request("/api/v2/responses", &principal, body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(response_id.get_or_insert(id.clone()), &id);
    }
}
#[tokio::test]
async fn binding_aliases_and_agent_ids_share_receipts_and_conceal_foreign_conversations() {
    let (app, store, bindings, principal) = fixture().await;
    let mut response_id = None;
    for (path, alias) in [
        ("/api/v2/responses", "conversation-external"),
        (
            "/api/v2/agents/agt_fixture/responses",
            "conversation-external",
        ),
        ("/api/v2/responses", "agt_fixture"),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                path,
                &principal,
                json!({"conversation":alias,"stream":true,"input":"Hello"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(response_id.get_or_insert(id.clone()), &id);
    }
    let other = ready(&store, self::binding("binding-foreign", &"b".repeat(64))).await;
    let other = bindings
        .authenticate(&other.key_sha256)
        .await
        .unwrap()
        .unwrap();
    for (principal, alias) in [
        (&principal, "conversation-missing"),
        (&other, "conversation-external"),
        (&other, "agt_fixture"),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "/api/v2/responses",
                principal,
                json!({"conversation":alias,"stream":true,"input":"Hello"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "conversation_not_found");
    }
}

#[tokio::test]
async fn retained_partition_with_no_agent_is_gone_without_reserving_a_response() {
    let (app, store, _, principal) = fixture().await;
    let binding = store
        .binding(principal.binding_id())
        .await
        .unwrap()
        .unwrap();
    let mut conversation = BindingConversation::new(
        &binding,
        "conversation-missing-agent".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(1), conversation.clone())
        .await
        .unwrap();
    let response = app
        .oneshot(request(
            "/api/v2/responses",
            &principal,
            json!({"conversation":conversation.conversation_id,"stream":true,"input":"alert"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    assert!(response.headers().get("retry-after").is_none());
    assert!(response.headers().get("x-cma-response-id").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "partition_agent_missing");
    assert!(store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn stopped_and_deleted_partition_agents_are_explicit_permanent_refusals() {
    let (app, store, _, principal) = fixture().await;
    let mut agent = store
        .get_agent(&layer2_core::AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        let revision = agent.revision;
        let reason = (status == AgentStatus::Stopped)
            .then_some(layer2_core::sandbox::AgentStopReason::Owner);
        agent
            .transition(status, layer2_core::now_rfc3339(), reason)
            .unwrap();
        store.save_agent(revision, agent.clone()).await.unwrap();
        let response = app
            .clone()
            .oneshot(request(
                "/api/v2/responses",
                &principal,
                json!({"conversation":"conversation-external","stream":true,"input":"alert"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(response.headers().get("retry-after").is_none());
        assert!(response.headers().get("x-cma-response-id").is_none());
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "partition_agent_unavailable");
    }
}

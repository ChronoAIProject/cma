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
    resource_access::*,
    sandbox::{agent_io::*, store::InMemorySandboxStore, AgentStatus},
    Caller, SubjectId,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct BindingAuthority(Arc<BindingAccessService>);
#[async_trait]
impl AgentCommandAdmissionAuthority for BindingAuthority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        _: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let principal = self
            .0
            .authenticate(&context.credential.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            *owner,
            AgentOwner::Person {
                subject: principal.owner().clone()
            }
        );
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
    let value = json!({"schema_version":1,
        "metadata":{"name":"responses", "owner":{"kind":"human","subject":"owner-a"},"provider":"registered"},
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
    let (state, store) = fixtures::conversation_fixture().await;
    let agent = fixtures::seeded_agent("agt_fixture", AgentStatus::Ready, 2, None);
    fixtures::insert_agent(&store, agent.clone(), vec![]).await;
    let mut reserved = binding("binding-responses", &"a".repeat(64));
    reserved.sandbox_id = agent.sandbox_id.clone();
    reserved.workspace_id = agent.workspace_id.clone();
    let binding = ready(&store, reserved).await;
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
    conversation.state = conversation_state;
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
    let authority = Arc::new(BindingAuthority(bindings.clone()));
    let access = Arc::new(
        ResourceAccessService::new(store.clone(), authority.clone())
            .with_bindings(bindings.clone()),
    );
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let app = super::router(
        super::ResponsesV2State::new(commands, state.reads.with_resource_access(access))
            .with_bindings(bindings.clone()),
    );
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
    assert_eq!(response.headers()["retry-after"], "1");
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

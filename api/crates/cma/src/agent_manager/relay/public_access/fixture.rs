use super::*;
use crate::auth::test_resources as resources;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use layer2_core::{agent_ownership::*, resource_access::*, sandbox::*, Caller, SubjectId};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Fixture {
    pub access: Arc<ResourceAccessService>,
    pub policy: Arc<PublicAccess>,
    pub store: Arc<store::InMemorySandboxStore>,
}
pub(crate) async fn fixture(auth: Arc<crate::auth::AuthState>, id: &str, owner: &str) -> Fixture {
    let store = Arc::new(store::InMemorySandboxStore::default());
    store
        .create_sandbox(resources::sandbox("sbx-public", owner))
        .await
        .unwrap();
    store
        .create_workspace(resources::workspace("wsp-public", "sbx-public", owner))
        .await
        .unwrap();
    store
        .create_worktree(resources::worktree(
            "wkt-public",
            "wsp-public",
            "sbx-public",
            id,
            owner,
        ))
        .await
        .unwrap();
    let mut agent = resources::agent(
        id,
        "wkt-public",
        "wsp-public",
        "sbx-public",
        owner,
        AgentStatus::Ready,
    );
    agent.conversation.conversation_id = Some("public-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.validate_conversation().unwrap();
    store.create_agent(agent).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
    let reads = AgentReadService::new(store.clone(), Arc::new(NoResources))
        .with_resource_access(access.clone());
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    Fixture {
        policy: PublicAccess::new(auth, reads, access.clone(), Arc::new(adapter)),
        store,
        access,
    }
}
struct NoResources;
#[async_trait::async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("preflight cannot access runtime resources")
    }
}
pub(super) fn auth(subject: &str) -> Arc<crate::auth::AuthState> {
    crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Trusted {
            subject: subject.into(),
        },
        None,
    )
}
pub(super) fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        caller: Caller::new(
            subject.clone(),
            crate::auth::AuthState::trusted_authorization(),
        ),
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "test".into(),
            subject: subject.clone(),
            credential_id: "test-credential".into(),
        },
        actor: ResourceActor::Api {
            subject,
            delegation_id: None,
        },
        memberships: vec![],
    }
}

#[derive(Default)]
pub(super) struct CountedStore {
    pub inner: test_support::MemoryOwnershipStore,
    pub calls: AtomicUsize,
}
impl CountedStore {
    fn touched(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl OwnershipStore for CountedStore {
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError> {
        self.inner.register(member).await
    }
    async fn heartbeat(&self, id: &str, drain: bool) -> Result<Membership, OwnershipError> {
        self.touched();
        self.inner.heartbeat(id, drain).await
    }
    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.touched();
        self.inner.membership().await
    }
    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.touched();
        self.inner.owner(id).await
    }
    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.touched();
        self.inner.has_owner_history(id).await
    }
    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.touched();
        self.inner.acquire(id, instance).await
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.touched();
        self.inner.renew(token).await
    }
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.touched();
        self.inner.release(token).await
    }
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.touched();
        self.inner.check(token).await
    }
}

pub(super) async fn router(f: &Fixture) -> (axum::Router, Arc<CountedStore>) {
    let store = Arc::new(CountedStore::default());
    let coordinator = OwnershipCoordinator::register(
        store.clone(),
        Member {
            instance_id: "public-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let updates = DurableAgentUpdateRecorder::new(f.store.clone());
    let relay = super::super::Relay::new(
        coordinator,
        &updates,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"public-policy-test"),
    )
    .unwrap();
    relay.enable_settings_owner_routing();
    relay.install_public_access(f.policy.clone()).unwrap();
    async fn denied() -> StatusCode {
        panic!("denied public caller reached effect handler")
    }
    let router = relay.routes(
        axum::Router::new()
            .route(
                "/api/v1/agents/{agent}/{*operation}",
                axum::routing::any(denied),
            )
            .route(
                "/api/v1/agents/{id}/files/content",
                axum::routing::get(denied),
            ),
    );
    (router, store)
}

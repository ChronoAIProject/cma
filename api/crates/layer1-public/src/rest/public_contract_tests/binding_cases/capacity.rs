//! The real binding handler receives a refusal at fresh persistence admission.
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use layer2_core::profiles::{store::InMemoryProfileStore, AgentProvider, ProfileCapacityBounds};
use layer2_core::resource_access::*;
use layer2_core::sandbox::binding::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentId, CoreError, SandboxId, ServiceId, SubjectId, WorkspaceId};

#[derive(Clone, Copy)]
pub(super) enum TransferOutcome {
    Reserved,
    Missing,
    Conflict,
    Unavailable,
}

struct ContractBindings {
    transfer: TransferOutcome,
}

macro_rules! unreachable_port {
    ($port:ident { $(fn $method:ident($($arg:ident: $kind:ty),*) -> $result:ty;)* }) => {
        #[async_trait]
        impl $port for ContractBindings {
            $(async fn $method(&self, $($arg: $kind),*) -> Result<$result, SandboxStoreError> {
                let _ = ($($arg,)*);
                panic!("fresh capacity refusal must precede resource effects and reads")
            })*
        }
    };
}

unreachable_port!(SandboxStore {
    fn create_sandbox(value: Sandbox) -> ();
    fn get_sandbox(id: &SandboxId) -> Option<Sandbox>;
    fn save_sandbox(revision: u64, value: Sandbox) -> ();
    fn list_sandboxes(owner: &SubjectId, page: &PageRequest) -> Page<Sandbox>;
});
unreachable_port!(WorkspaceStore {
    fn create_workspace(value: Workspace) -> ();
    fn get_workspace(id: &WorkspaceId) -> Option<Workspace>;
    fn save_workspace(revision: u64, value: Workspace) -> ();
    fn list_workspaces(id: &SandboxId, page: &PageRequest) -> Page<Workspace>;
});
unreachable_port!(AgentStore {
    fn create_agent(value: Agent) -> ();
    fn get_agent(id: &AgentId) -> Option<Agent>;
    fn save_agent(revision: u64, value: Agent) -> ();
    fn list_agents(owner: &SubjectId, page: &PageRequest) -> Page<Agent>;
});

#[async_trait]
impl ResourceAccessStore for ContractBindings {
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        unreachable!()
    }
}

#[async_trait]
impl BindingStore for ContractBindings {
    async fn transfer_profile_conversation(
        &self,
        request: ProfileTransferRequest,
        enabled: bool,
    ) -> Result<ProfileTransferReceipt, BindingError> {
        assert!(enabled);
        assert_eq!(request.manager.as_str(), "manager");
        assert_eq!(request.source_binding_id, "binding-source");
        assert_eq!(request.destination_binding_id, "binding-successor");
        assert_eq!(request.conversation_id, "conversation-contract");
        assert_eq!(request.expected_agent_id.as_str(), "agt_original");
        assert_eq!(request.expected_generation, 1);
        assert_eq!(request.expected_destination_generation, 2);
        assert_eq!(request.expected_incarnation, 0);
        assert_eq!(request.operation_id, "b".repeat(64));
        assert_eq!(request.request_digest, "a".repeat(64));
        assert_eq!(
            request.owner,
            ResourceOwner::Person {
                subject: SubjectId::new("owner")
            }
        );
        match self.transfer {
            TransferOutcome::Reserved => {}
            TransferOutcome::Missing => return Err(BindingError::NotFound),
            TransferOutcome::Conflict => return Err(BindingError::Conflict),
            TransferOutcome::Unavailable => {
                return Err(BindingError::Unavailable(
                    "fixture store unavailable".into(),
                ))
            }
        }
        Ok(ProfileTransferReceipt {
            source_conversation_id: request.conversation_id.clone(),
            destination_generation: request.expected_destination_generation,
            destination_agent_id: AgentId("agt_successor".into()),
            destination_worktree_id: layer2_core::WorktreeId("wkt_successor".into()),
            request,
        })
    }
    async fn binding(&self, _: &str) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        Ok(None)
    }
    async fn save_binding(
        &self,
        expected: Option<u64>,
        next: ProviderBinding,
    ) -> Result<(), SandboxStoreError> {
        assert!(expected.is_none());
        assert!(next.existing_agent.is_none());
        assert!(next.profile.environment.is_some());
        Err(SandboxStoreError::GlobalCapacityReached)
    }
    async fn contains_key_hash(&self, _: &str) -> Result<bool, SandboxStoreError> {
        unreachable!()
    }
    async fn find_by_key_hash(
        &self,
        _: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation_for_agent(
        &self,
        _: &str,
        _: u64,
        _: &AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn pending_bindings(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation(
        &self,
        _: &str,
        _: u64,
        _: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn save_binding_conversation(
        &self,
        _: u64,
        _: Option<u64>,
        _: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversations(
        &self,
        _: &str,
        _: u64,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn pending_binding_conversations(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
}

struct NeverRetain;
#[async_trait]
impl BindingCredentialCustody for NeverRetain {
    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        panic!("refused bindings must not retain credentials")
    }
    async fn retain_binding(
        &self,
        _: &ProviderBinding,
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        panic!("refused bindings must not retain credentials")
    }
}

struct NeverResolve;
#[async_trait]
impl AgentRuntimeRegistry for NeverResolve {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        panic!("refused bindings must not contact a runtime")
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("refused bindings must not contact a runtime")
    }
}

pub(super) fn fixture() -> Router {
    crate::rest::binding_router(service(TransferOutcome::Reserved))
}

pub(super) fn profile_transfer_fixture(outcome: TransferOutcome) -> Router {
    crate::rest::binding_conversation_reset_router(service(outcome), true)
}

fn service(transfer: TransferOutcome) -> Arc<BindingService> {
    let resources = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    BindingService::new(
        Arc::new(ContractBindings { transfer }),
        BindingResources {
            sandboxes: SandboxService::new(
                resources.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
            ),
            workspaces: WorkspaceService::new(resources.clone(), profiles.clone()),
            agents: AgentService::new(
                resources,
                profiles,
                Arc::new(NeverResolve),
                layer2_core::RuntimeCapabilities::new([
                    layer2_core::RuntimeCapability::Conversation,
                ]),
            ),
        },
        Arc::new(NeverRetain),
        BindingPolicy {
            provider: AgentProvider::default(),
            supported_model_api: "responses".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            sandbox: Arc::new(SandboxPolicy::default()),
        },
    )
}

#[tokio::test]
async fn fresh_binding_put_preserves_the_store_capacity_refusal() {
    use tower::ServiceExt;
    let response = fixture()
        .oneshot(super::management::request(
            "PUT",
            "/api/v1/cmaeg/bindings/binding-contract",
            Some(super::management::body()),
            Some(super::management::authority()),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(response.headers()["retry-after"], "60");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "sandbox_capacity_unavailable");
    assert_eq!(
        body["detail"],
        layer2_core::admission::CapacityKind::RetainedHomes.guidance()
    );
    assert_eq!(body.as_object().unwrap().len(), 5);
}

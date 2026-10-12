//! Exercise CMA's real binding negotiation while keeping runtime allocation local.
use layer2_core::{
    profiles::{store::InMemoryProfileStore, ProfileCapacityBounds},
    sandbox::{binding::*, *},
    CoreError, SandboxId, ServiceId, SubjectId,
};
use std::sync::Arc;

struct Registry;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Err(AgentRuntimeError::NotFound)
    }
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}
struct Custody;
#[async_trait::async_trait]
impl BindingCredentialCustody for Custody {
    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        unreachable!("binding-specific custody must be used")
    }
    async fn retain_binding(
        &self,
        binding: &ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        binding.require_credential_admission(credential)
    }
}
pub(super) fn service() -> Arc<BindingService> {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let resources = BindingResources {
        sandboxes: SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
        ),
        workspaces: WorkspaceService::new(store.clone(), profiles.clone()),
        agents: AgentService::new(
            store.clone(),
            profiles.clone(),
            Arc::new(Registry),
            layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
        ),
    };
    BindingService::new(
        store,
        resources,
        Arc::new(Custody),
        BindingPolicy {
            provider: Default::default(),
            supported_model_api: "responses".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 1,
                max_cpu_millis: 64000,
                min_memory_mib: 1,
                max_memory_mib: 131072,
            },
            sandbox: Arc::new(SandboxPolicy::default()),
        },
    )
}

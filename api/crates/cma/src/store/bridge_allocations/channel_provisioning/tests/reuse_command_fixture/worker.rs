use super::*;
use crate::sandbox::{
    channel_credentials::ChannelGuardedCredentials,
    worker::{
        CheckpointObjectPurger, LiveAgentStopper, ProfileWorkspaceProvisioner,
        SandboxDependencyError, SandboxWorkerConfig, SandboxWorkerDependencies,
    },
    ManagedSandboxCredentials, OwnerTokenSource,
};

struct NoNewEffects;
#[async_trait::async_trait]
impl OwnerTokenSource for NoNewEffects {
    async fn token_for(
        &self,
        _: &layer2_core::SubjectId,
    ) -> Result<Option<crate::auth::persistence::SecretValue>, String> {
        panic!("short ordinary-parent revival must retain its custodied credential")
    }
}
#[async_trait::async_trait]
impl ProfileWorkspaceProvisioner for NoNewEffects {
    async fn create_profile_workspace(
        &self,
        _: &Sandbox,
        _: &RuntimeHandle,
        _: &layer2_core::profiles::WorkspaceProfileRef,
        _: u16,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("revival must preserve existing children")
    }
}
#[async_trait::async_trait]
impl LiveAgentStopper for NoNewEffects {
    async fn stop_live_agents(
        &self,
        _: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("revival cannot retire siblings")
    }
}
#[async_trait::async_trait]
impl CheckpointObjectPurger for NoNewEffects {
    async fn purge_checkpoint_objects(
        &self,
        _: &Sandbox,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("revival cannot purge checkpoints")
    }
}

pub(super) async fn publish(f: &ReuseFixture, record: &AllocationRecord) -> Arc<LiveAgentRegistry> {
    crate::sandbox::channel_startup::test_support::publish_connections(
        f.original.resources.clone(),
        f.original.adapter.clone(),
        &[
            f.sibling.id.clone(),
            record.placement.target().agent_id.clone(),
        ],
    )
    .await
}

pub(super) fn build(f: &ReuseFixture, registry: Arc<LiveAgentRegistry>) -> Arc<SandboxWorker> {
    let ordinary = ManagedSandboxCredentials::new(
        f.original.server.issuer.clone(),
        Arc::new(NoNewEffects),
        f.original.custody.clone(),
    );
    // Real provenance guard, no Channel-root lifecycle driver installed.
    let credentials = ChannelGuardedCredentials::new(ordinary, f.original.allocations.clone());
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: f.original.resources.clone(),
            adapter: f.original.adapter.clone(),
            credentials,
            workspaces: Arc::new(NoNewEffects),
            agents: Arc::new(NoNewEffects),
            checkpoints: Arc::new(NoNewEffects),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(f.original.resources.clone()),
            operations: AgentOperationService::new(f.original.resources.clone(), registry),
        },
        SandboxWorkerConfig::new(
            "ordinary-reused-parent-lifecycle",
            "fixture-deployment",
            f.original.settings.runtime_version.clone(),
            SandboxPolicy::default(),
        ),
    )
}

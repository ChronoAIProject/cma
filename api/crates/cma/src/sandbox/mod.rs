//! Sandbox lifecycle orchestration below the public API boundary.

mod agent_scheduler;
mod agent_skills;
mod agent_worker;
#[allow(
    dead_code,
    reason = "cleanup request seal awaits effect and restore integration"
)]
mod checkpoint_maintenance;
mod composition;
mod credential;
pub(crate) mod funding;
pub(crate) use credential::owner_tokens::ServiceAccessOwnerTokens;
mod home;
mod home_native;
mod live_agent_stopper;
mod live_agents;
pub(crate) mod native_prepared;
mod policy;
mod provenance_backfill;
pub(crate) use provenance_backfill::AgentProvenanceBackfill;
mod reconciliation;
pub(crate) mod retained_recovery;
pub(crate) mod runtime_attachment;
mod runtime_mapping;
mod scheduler;
mod suggestion_worker;
pub(crate) mod suggestions;
mod title_budget;
mod title_scheduler;
mod title_worker;
mod titles;
mod unavailable_credentials;
pub(crate) mod unpublished_recovery;
mod warmup;
pub(crate) mod worker;
mod workspace_worker;

pub(crate) use agent_skills::OrnnAgentSkillResolver;
pub(crate) use composition::{
    SandboxLifecycleComposition, SandboxLifecycleConfig, SandboxLifecycleDependencies,
};
pub(crate) use credential::{
    CredentialCustodyError, CredentialError, CredentialMaterial, CredentialRevocation,
    ManagedSandboxCredentials, OwnerTokenSource, SandboxCredentialController,
    SandboxCredentialCustody,
};
pub(crate) use live_agents::LiveAgentRegistry;
pub(crate) use policy::LiveSandboxPolicy;
pub(crate) use runtime_mapping::{runtime_capabilities, runtime_options};
pub(crate) use title_worker::PlatformTitleEndpoint;
pub(crate) use unavailable_credentials::UnavailableSandboxCredentials;

#[cfg(test)]
pub(crate) use reconciliation::SandboxReconciler;
#[cfg(test)]
#[cfg(test)]
mod agent_worker_tests;
#[cfg(test)]
mod worker_tests;

pub(crate) mod legacy_credentials;

#[cfg(test)]
pub(crate) mod binding_lifecycle_test_support {
    pub(crate) use super::agent_skills::EmptyAgentSkillResolver;
    pub(crate) use super::agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies};
    pub(crate) use super::live_agent_stopper::DurableLiveAgentStopper;
    pub(crate) use super::workspace_worker::{
        DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
        WorkspaceWorkerDependencies,
    };
}

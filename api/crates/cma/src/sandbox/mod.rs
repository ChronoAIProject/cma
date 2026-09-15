//! Sandbox lifecycle orchestration below the public API boundary.

mod agent_scheduler;
mod agent_skills;
mod agent_worker;
pub(crate) mod channel_children;
pub(crate) mod channel_credentials;
pub(crate) mod channel_lifecycle_credentials;
pub(crate) mod channel_mint_reconciliation;
pub(crate) mod channel_renewal;
pub(crate) mod channel_retirement;
pub(crate) mod channel_reuse;
mod checkpoint_maintenance;
pub(crate) use checkpoint_maintenance::ExportDependencies;
pub(crate) mod channel_provisioning;
pub(crate) mod channel_startup;
mod checkpoint_purger;
mod composition;
mod credential;
mod home;
mod live_agent_stopper;
mod live_agents;
mod policy;
mod provenance_backfill;
pub(crate) use provenance_backfill::AgentProvenanceBackfill;
mod reconciliation;
mod runtime_attachment;
mod runtime_mapping;
mod scheduler;
mod title_scheduler;
mod title_worker;
mod titles;
mod unavailable_credentials;
mod warmup;
pub(crate) mod worker;
mod workspace_worker;

pub(crate) use agent_skills::OrnnAgentSkillResolver;
pub(crate) use checkpoint_purger::DurableCheckpointObjectPurger;
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
pub(crate) use title_worker::ProviderTitleEndpoint;
pub(crate) use unavailable_credentials::UnavailableSandboxCredentials;

#[cfg(test)]
mod agent_worker_tests;
#[cfg(test)]
mod worker_tests;

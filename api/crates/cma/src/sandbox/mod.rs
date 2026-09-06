//! Sandbox lifecycle orchestration below the public API boundary.

mod agent_skills;
mod agent_worker;
mod checkpoint_purger;
mod composition;
mod credential;
mod live_agent_stopper;
mod live_agents;
mod policy;
mod reconciliation;
mod runtime_attachment;
mod runtime_mapping;
mod scheduler;
#[allow(
    dead_code,
    reason = "composed when the production agent-title worker resolves sandbox proxy endpoints"
)]
mod titles;
mod unavailable_credentials;
mod worker;
mod workspace_worker;

pub(crate) use agent_skills::OrnnAgentSkillResolver;
pub(crate) use checkpoint_purger::DurableCheckpointObjectPurger;
pub(crate) use composition::{
    SandboxLifecycleComposition, SandboxLifecycleConfig, SandboxLifecycleDependencies,
};
pub(crate) use credential::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, ManagedSandboxCredentials,
    SandboxCredentialController, SandboxCredentialCustody,
};
pub(crate) use live_agents::LiveAgentRegistry;
pub(crate) use policy::LiveSandboxPolicy;
pub(crate) use runtime_mapping::runtime_capabilities;
pub(crate) use unavailable_credentials::UnavailableSandboxCredentials;

#[cfg(test)]
mod agent_worker_tests;
#[cfg(test)]
mod worker_tests;

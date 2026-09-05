//! Sandbox lifecycle orchestration below the public API boundary.

#[allow(
    dead_code,
    reason = "composed when the production agent lifecycle transport is available"
)]
mod agent_skills;
#[allow(
    dead_code,
    reason = "composed when the production agent lifecycle transport is available"
)]
mod agent_worker;
mod credential;
#[allow(
    dead_code,
    reason = "composed when the production agent lifecycle transport is available"
)]
mod live_agents;
mod policy;
mod runtime_mapping;
#[allow(
    dead_code,
    reason = "started when the production hosted-control transport is composed"
)]
mod scheduler;
#[allow(
    dead_code,
    reason = "composed when the production agent-title worker resolves sandbox proxy endpoints"
)]
mod titles;
#[allow(
    dead_code,
    reason = "composed when the production hosted-control transport supplies an adapter"
)]
mod worker;
#[allow(
    dead_code,
    reason = "composed when the production hosted-control transport supplies an adapter"
)]
mod workspace_worker;

pub(crate) use credential::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, ManagedSandboxCredentials,
    SandboxCredentialCustody,
};
pub(crate) use live_agents::LiveAgentRegistry;
pub(crate) use policy::LiveSandboxPolicy;
pub(crate) use runtime_mapping::runtime_capabilities;

#[cfg(test)]
mod agent_worker_tests;
#[cfg(test)]
mod worker_tests;

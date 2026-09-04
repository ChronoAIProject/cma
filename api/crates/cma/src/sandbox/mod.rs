//! Sandbox lifecycle orchestration below the public API boundary.

mod credential;
mod policy;
#[allow(
    dead_code,
    reason = "started when the production hosted-control transport is composed"
)]
mod scheduler;
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
pub(crate) use policy::LiveSandboxPolicy;

#[cfg(test)]
mod worker_tests;

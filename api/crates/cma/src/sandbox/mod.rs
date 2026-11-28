//! Sandbox lifecycle orchestration below the public API boundary.

pub(crate) mod agent_process_proof;
mod agent_scheduler;
mod agent_skills;
mod agent_worker;
mod capacity_scheduler;
#[allow(
    dead_code,
    reason = "cleanup request seal awaits effect and restore integration"
)]
mod checkpoint_maintenance;
mod composition;
mod credential;
pub(crate) mod funding;
mod lifecycle_failure;
pub(crate) mod ownership_retry_hint;
pub(crate) use credential::owner_tokens::ServiceAccessOwnerTokens;
mod home;
mod home_native;
mod live_agent_stopper;
mod live_agents;
pub(crate) mod native_prepared;
mod policy;
mod provenance_backfill;
pub(crate) use provenance_backfill::AgentProvenanceBackfill;
mod model_request_budget;
pub(crate) mod native_retirement_delivery;
mod native_retirement_finalizer;
mod reconciliation;
pub(crate) mod retained_recovery;
pub(crate) mod retained_runtime_service;
pub(crate) mod runtime_attachment;
mod runtime_mapping;
mod scheduler;
mod stale_turn_reconciliation;
#[cfg(test)]
pub(crate) use stale_turn_reconciliation::assert_partial_does_not_request_stop;
mod suggestion_worker;
pub(crate) mod suggestions;
mod title_budget;
mod title_scheduler;
mod title_worker;
pub(crate) mod titles;
pub(crate) mod turn_timeout_worker;
mod unavailable_credentials;
pub(crate) mod unpublished_recovery;
mod warmup;
pub(crate) mod work_retry;
pub(crate) mod worker;
mod workspace_worker;

pub(crate) use agent_skills::OrnnAgentSkillResolver;
pub(crate) use composition::{
    SandboxLifecycleComposition, SandboxLifecycleConfig, SandboxLifecycleDependencies,
};
pub(crate) use credential::{
    CredentialCustodyError, CredentialError, CredentialIssuance, CredentialMaterial,
    CredentialRevocation, IssuanceIdentity, IssuanceObservation, IssuanceState,
    ManagedSandboxCredentials, OwnerTokenSource, SandboxCredentialController,
    SandboxCredentialCustody,
};
#[cfg(test)]
pub(crate) use live_agents::record_update_for_test;
pub(crate) use live_agents::{LiveAgentRegistry, LiveAgentTarget};
#[cfg(test)]
pub(crate) use model_request_budget::tests::assert_metered_auxiliary_refusal;
#[cfg(test)]
pub(crate) use model_request_budget::tests::durable_http::{auxiliary_request, AuxiliaryKind};
pub(crate) use policy::LiveSandboxPolicy;
pub(crate) use runtime_mapping::{runtime_capabilities, runtime_options};
pub(crate) use title_worker::PlatformTitleEndpoint;
#[cfg(test)]
pub(crate) use title_worker::{scheduled_title_fixture, start_title_scheduler};
pub(crate) use unavailable_credentials::UnavailableSandboxCredentials;

#[cfg(test)]
pub(crate) use reconciliation::SandboxReconciler;
#[cfg(test)]
#[cfg(test)]
mod agent_worker_tests;
#[cfg(test)]
mod worker_tests;
#[cfg(test)]
pub(crate) use worker_tests::exercise_native_retirement_finality;
#[cfg(test)]
pub(crate) use worker_tests::exercise_runtime_update_reopen_http;
#[cfg(test)]
pub(crate) use worker_tests::{
    seed_sandbox_profile_with_capacity, ActionLog, AgentStopRecorder, MemoryCredentials,
    WorkspaceRecorder,
};

pub(crate) mod legacy_credentials;

#[cfg(test)]
pub(crate) mod binding_lifecycle_test_support {
    pub(crate) use super::agent_skills::EmptyAgentSkillResolver;
    pub(crate) use super::agent_worker::{
        AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, ConnectionSweep,
    };
    pub(crate) use super::agent_worker::{
        CapacityRecoveryReconnect, ReconnectAction, RecoveryReconnect,
    };
    pub(crate) use super::live_agent_stopper::DurableLiveAgentStopper;
    pub(crate) use super::live_agents::{
        spawn_update_task, DurableAgentConnectionLossHandler, LiveAgentAttachmentIdentity,
        LiveAgentConnectionLossHandler,
    };
    pub(crate) use super::workspace_worker::{
        DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
        WorkspaceWorkerDependencies,
    };
}

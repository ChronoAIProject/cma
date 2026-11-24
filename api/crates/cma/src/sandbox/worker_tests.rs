use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use layer2_core::profiles::store::{InMemoryProfileStore, WorkspaceProfileStore as _};
use layer2_core::profiles::{
    AgentProvider, EnvironmentVariable, ModelAccess, ProfileCapacityBounds, ProfileVisibility,
    SandboxProfile, SandboxProfileContent, SandboxProfileRef, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent, WorkspaceProfileRef,
};
use layer2_core::sandbox::store::{
    PageRequest, SandboxEventStore as _, SandboxLifecycleCommand,
    SandboxLifecycleCommandStore as _, SandboxStore as _, SandboxStoreError,
    SandboxWorkerStore as _,
};
use layer2_core::sandbox::{
    AgentOperationService, CreateSandbox, DurableAgentUpdateRecorder, InMemorySandboxStore,
    Sandbox, SandboxEventKind, SandboxOrigin, SandboxPolicy, SandboxPolicySource,
    SandboxReviveCause, SandboxRotationIdentity, SandboxService, SandboxStatus, SandboxWorkStep,
};
use layer2_core::{
    rfc3339_at, seconds_from_rfc3339, Authorization, Caller, Capability, SandboxId,
    SandboxProfileId, ServiceId, StoreError, SubjectId, WorkspaceProfileId,
};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use layer3_adapter::wire::{RequestFrame, WireOperation};
use layer3_adapter::{RuntimeHandle, SandboxAdapter};
use tokio::sync::Notify;

use crate::auth::persistence::SecretValue;

use super::credential::{CredentialError, CredentialMaterial, SandboxCredentialController};
use super::live_agents::LiveAgentRegistry;
use super::worker::{
    LiveAgentStopper, ProfileWorkspaceProvisioner, SandboxDependencyError, SandboxWorker,
    SandboxWorkerConfig, SandboxWorkerDependencies, CREDENTIAL_EXPIRY_MARGIN,
    CREDENTIAL_ROTATION_AGE,
};

mod background_checkpoint_tests;
mod binding_tests;
mod checkpoint_consistency_tests;
mod checkpoint_lease_tests;
mod checkpoint_maintenance_tests;
mod execution_tests;
mod funding_tests;
mod home_restore_reader_tests;
mod home_tests;
mod hot_scope_tests;
mod lifecycle_failure_clock_tests;
mod lifecycle_queue_conflict_tests;
mod lifecycle_queue_contention_tests;
mod lifecycle_queue_tests;
mod lifecycle_recovery_tests;
mod native_pool_tests;
mod native_suspension_tests;
pub(crate) use native_suspension_tests::exercise_native_retirement_finality;
pub(crate) use native_suspension_tests::exercise_runtime_update_reopen_http;
mod owned_reopen_tests;
mod pause_cancellation_tests;
mod provided_credential_tests;
mod reconciliation_tests;
mod retirement_checkpoint_tests;
#[path = "worker_tests/reviewer_boundary_diagnostics.rs"]
mod reviewer_boundary_diagnostics;
mod runtime_attachment_tests;
mod startup_hold_tests;
mod warmup_tests;
mod warmup_transfer_tests;
mod work_authority_tests;
mod workspace_adoption_tests;
mod workspace_prepare_recovery_tests;

const PROFILE_TIME: &str = "2026-09-05T00:00:00Z";

fn runtime_image() -> String {
    CodexSandboxSpec::default().deployment.image.uri
}

#[derive(Clone, Default)]
pub(crate) struct ActionLog(Arc<Mutex<Vec<String>>>);

impl ActionLog {
    fn push(&self, action: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(action.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct MutablePolicy(Mutex<SandboxPolicy>);

#[async_trait::async_trait]
impl SandboxPolicySource for MutablePolicy {
    async fn current_sandbox_policy(&self) -> Result<SandboxPolicy, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }
}

mod credential_fixture;
pub(crate) use credential_fixture::MemoryCredentials;
use credential_fixture::*;
mod harness;
use harness::*;
pub(crate) use harness::{AgentStopRecorder, WorkspaceRecorder};
mod profile_fixtures;
pub(crate) use profile_fixtures::seed_sandbox_profile_with_capacity;
use profile_fixtures::*;
mod base_creation;
mod base_credential_rotation;
mod base_failures;
mod base_lease;
mod base_retirement;
mod retirement_credentials_tests;

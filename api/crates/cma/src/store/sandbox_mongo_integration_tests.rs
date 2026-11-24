use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::store::{
    AgentProfileStore, ProfilePageRequest, ProfileStoreError, WorkspaceProfileStore,
};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess,
    PermissionLevel, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent,
    WorkspaceProfileRef, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentConnectionRecoveryCause,
    AgentConnectionRecoveryOutcome, AgentConnectionRecoveryRequest, AgentControlResult,
    AgentControlStore, AgentControlStoreError, AgentControlTarget, AgentConversationMutation,
    AgentConversationStore, AgentDeleteRequest, AgentLogStore, AgentStopRequest, AgentStore,
    AgentWorkMutation, AgentWorkerStore, PageRequest, SandboxAdmission, SandboxAdmissionName,
    SandboxAdmissionResult, SandboxAdmissionStore, SandboxEventStore, SandboxStore,
    SandboxStoreError, SandboxTransaction, SandboxTransactionStore, SandboxWorkerStore,
    VersionedWorktree, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionResult,
    WorkspaceAdmissionStore, WorkspaceStore, WorkspaceWorkerStore, WorktreeStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentOrigin, AgentStatus, AgentStopReason,
    AgentWorkReceipt, AgentWorkState, AgentWorkStep, ConversationState, CreateWorkspace, Sandbox,
    SandboxCredentialAuthority, SandboxCredentialState, SandboxEvent, SandboxEventKind,
    SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState,
    SandboxWorkStep, TypedFailure, Workspace, WorkspaceOrigin, WorkspaceService,
    WorkspaceServiceError, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
    Worktree, WorktreeStatus,
};
use layer2_core::{Authorization, Caller, Capability};
use mongodb::bson::{doc, Bson, Document};
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use mongodb::options::{ClientOptions, ReadConcern};
use mongodb::Client;

use crate::auth::persistence::SecretValue;
use crate::sandbox::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, SandboxCredentialCustody,
};

use super::agent_control_lineage::{fence_control_lineage, load_control_snapshot, LineageError};
use super::agent_control_receipt::{
    read_control_receipts, ControlOperation, ControlRequest, DELETE_RECEIPT_FIELD,
    STOP_RECEIPT_FIELD,
};
use super::agent_control_store::{plan_delete, plan_stop};
use super::agent_control_transaction::{
    control_snapshot_matches, reconciliation_error, same_agent_incarnation,
};
use super::agent_conversation_store::{
    reconcile_conversation, reconcile_conversation_snapshot, validate_mutation, ConversationReceipt,
};
use super::agent_log::log_document;
use super::agent_log_integrity::AppendCapacity;
use super::agent_worker_capacity::publication_capacity;
use super::agent_worker_reconcile::reconcile_publication;
use super::agent_worker_transaction::PublicationReceipt;
use super::agents::agent_replacement_fields;
use super::encryption::AuthEncryptionKey;
use super::sandbox_credentials::{
    MongoSandboxCredentialStore, SANDBOX_CREDENTIALS, SANDBOX_CREDENTIAL_REVOCATIONS,
};
use super::sandbox_events::event_document;
use super::sandbox_mongo::{
    is_transient, start_transaction, MongoProfileStore, MongoSandboxStore, AGENTS, AGENT_LOG,
    AGENT_PROFILES, AGENT_PROFILE_REVISIONS, CHECKPOINTS, DOCUMENT_SCHEMA_VERSION, SANDBOXES,
    SANDBOX_ADMISSIONS, SANDBOX_EVENTS, SANDBOX_PROFILES, SANDBOX_PROFILE_REVISIONS, WORKSPACES,
    WORKSPACE_PROFILES, WORKSPACE_PROFILE_REVISIONS, WORKTREES,
};
use super::worktrees::worktree_replacement_fields;
use super::DefaultProfileSpec;

const NOW: &str = "2026-09-05T00:00:00Z";
mod background_checkpoint_tests;
pub(super) mod capacity_fixtures;
pub(in crate::store) mod command_ledger_tests;
mod conversation_fixtures;
mod ledger_census_tests;
pub(in crate::store) mod ledger_retirement_tests;
mod live_sandbox_pages;
pub(super) mod native_home_fixture;
mod read_telemetry_tests;
mod released_runtime_revival_tests;
pub(in crate::store) mod resource_access_tests;
mod visible_inventory;
use conversation_fixtures::*;
const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Default)]
struct CommandTrace {
    started: Vec<(String, Document)>,
    failed_commits: usize,
    commit_replies: Vec<Document>,
}

mod common_fixtures;
use common_fixtures::*;
pub(super) use common_fixtures::{
    encryption_key, ready_conversation_fixture, ready_workspace, sandbox,
};

mod agent_capacity_recovery_tests;
mod agent_log_read_tests;
mod agent_segment_tests;
mod agent_title_tests;
pub(super) mod agent_transaction_tests;
mod agent_worker_tests;
mod checkpoint_capture_tests;
mod execution_tests;
mod launch_command_tests;
mod log_integrity_tests;
mod persistence_tests;
mod profile_transaction_tests;
mod response_projection_tests;
pub(in crate::store) mod sandbox_lifecycle_tests;
mod sandbox_usage_tests;
mod schema_admission_tests;
mod service_access_tests;
mod skill_material_tests;
mod task_title_tests;
pub(in crate::store) mod transaction_reconciliation_tests;
mod warm_hierarchy_tests;
mod workspace_admission_tests;
mod workspace_lifecycle_tests;

// Historical payload timestamps stay fixed; acquisition needs a live deadline.
pub(in crate::store) fn work_lease_deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
}

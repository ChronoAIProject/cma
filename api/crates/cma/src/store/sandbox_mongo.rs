//! Shared MongoDB boundary for the fresh sandbox-model schema.
use layer2_core::{sandbox::SandboxStoreError, StoreError};
use mongodb::bson::{doc, Document};
use mongodb::error::Error;
use mongodb::{Client, ClientSession, Collection};
use std::future::Future;
use std::time::Duration;
use tokio::sync::Semaphore;

mod handles;
mod records;
pub(super) use records::{
    i64_field_matches, nullable, read_record, record_bson, require_version, signed,
    string_field_matches, unsigned,
};

use super::MongoCheckpointObjectStore;
use super::{durability::majority_collection, schema::initialize_collections};

#[path = "sandbox_mongo_transactions.rs"]
mod transactions;
pub(crate) use transactions::{
    commit_transaction, is_confirmed_body_retry, is_duplicate_key, start_transaction, CommitOutcome,
};
pub(super) use transactions::{commit_transaction_with_refusal, is_transient, PrecommitRefusal};

pub use super::profile_mongo::MongoProfileStore;
pub(super) use super::schema::DOCUMENT_SCHEMA_VERSION;
mod topology;
pub(super) use topology::require_transaction_topology;
#[cfg(test)]
use topology::validate_transaction_server;

pub(super) const TRANSACTION_ATTEMPTS: usize = 5;
const RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PENDING_RECONCILIATIONS: usize = 16;
static RECONCILIATION_SLOTS: Semaphore = Semaphore::const_new(MAX_PENDING_RECONCILIATIONS);

pub(super) const SANDBOXES: &str = "sandboxes";
pub(super) const SANDBOX_SESSIONS: &str = "sandbox_sessions";
pub(super) const WORKSPACES: &str = "workspaces";
pub(super) const WORKTREES: &str = "worktrees";
pub(super) const AGENTS: &str = "agents";
pub(super) const AGENT_REUSE_BINDINGS: &str = "agent_reuse_bindings";
pub(super) const AGENT_LOG: &str = "agent_log";
pub(super) const AGENT_COMMANDS: &str = "agent_commands";
pub(super) const AGENT_ARTIFACTS: &str = "agent_artifacts";
pub(super) const SANDBOX_CONTROLS: &str = "sandbox_controls";
pub(super) const COMMAND_AUTHORIZATION_AUDITS: &str = "command_authorization_audits";
pub(super) const AGENT_COMMAND_ORDERS: &str = "agent_command_orders";
pub(super) const RESOURCE_POLICIES: &str = "resource_policies";
pub(super) const RESOURCE_SHARE_COMMANDS: &str = "resource_share_commands";
pub(super) const RESOURCE_SHARES: &str = "resource_shares";
pub(super) const SANDBOX_EVENTS: &str = "sandbox_events";
pub(super) const SANDBOX_ADMISSIONS: &str = "sandbox_admissions";
pub(super) const CHECKPOINTS: &str = "checkpoints";
pub(super) const AGENT_PROFILES: &str = "agent_profiles";
pub(super) const AGENT_PROFILE_REVISIONS: &str = "agent_profile_revisions";
pub(super) const WORKSPACE_PROFILES: &str = "workspace_profiles";
pub(super) const WORKSPACE_PROFILE_REVISIONS: &str = "workspace_profile_revisions";
pub(super) const SANDBOX_PROFILES: &str = "sandbox_profiles";
pub(super) const SANDBOX_PROFILE_REVISIONS: &str = "sandbox_profile_revisions";
pub(super) const WARM_SANDBOXES: &str = "warm_sandboxes";
pub(super) const SANDBOX_BILLING: &str = "sandbox_billing_state";

mod collections;
use collections::SANDBOX_COLLECTIONS;

/// One adapter implements every sandbox-model port so cross-port operations
/// share a client session and therefore a real Mongo transaction.
#[derive(Clone)]
pub struct MongoSandboxStore {
    pub(super) creation_writer_epoch: Option<String>,
    pub(super) usage_capture: Option<super::sandbox_usage_export::CaptureStore>,
    pub(super) initial_settings: super::settings_defaults::InitialSettings,
    pub(super) agent_inspection_parking: bool,
    pub(super) ownership_retry_hints_enabled: bool,
    #[cfg(test)]
    pub(super) ownership_retry_hint_now: Option<mongodb::bson::DateTime>,
    pub(super) empty_discovery: super::empty_discovery::Policy,
    pub(super) agent_connection_observations:
        Option<std::sync::Arc<super::agent_connection_observations::Observations>>,
    pub(super) segment_logs: super::agent_log_segments::SegmentCollections,
    pub(super) global_sandbox_limit: u16,
    pub(super) public_owner_sandbox_limit: Option<std::num::NonZeroU16>,
    #[cfg(test)]
    pub(super) request_budget_clock: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    pub(super) runtime_migration_enabled: bool,
    pub(super) runtime_migration_verifier:
        Option<std::sync::Arc<layer2_core::sandbox::RuntimeActivityVerifier>>,
    pub(super) runtime_migration_authority:
        Option<std::sync::Arc<dyn layer2_core::sandbox::RuntimeMigrationAuthority>>,
    pub(super) renewable_work: bool,
    pub(super) segment_adoption_enabled: bool,
    pub(super) bounded_approval_reads: bool,
    pub(super) runtime_loss_bounded_proof_enabled: bool,
    pub(super) bounded_close_retries: bool,
    pub(super) retry_single_marker_enabled: bool,
    pub(super) retry_audits:
        std::sync::Arc<super::agent_worker_transaction::retry_audit::RetryAudits>,
    pub(super) scan_cooperation: super::cooperative_budget::Policy,
    pub(super) fleet_log_activation: std::sync::Arc<super::agent_log_integrity::fleet::Activation>,
    pub(super) bounded_admission_agents: std::collections::BTreeSet<layer2_core::AgentId>,
    pub(super) platform_counter_shards: bool,
    pub(super) bounded_title_reads: bool,
    pub(super) title_effect_custody: bool,
    pub(super) bounded_transcript_reads: bool,
    pub(super) bounded_usage_reads: bool,
    pub(super) access_snapshot_session: bool,
    pub(super) resource_list_batch_reads: bool,
    pub(super) sandbox_agent_projection: bool,
    pub(super) sandbox_snapshot_reads: bool,
    pub(super) sandbox_batched_reads: bool,
    pub(super) bounded_response_reads: bool,
    pub(super) active_log_proofs: bool,
    pub(crate) writer_policy:
        std::sync::Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    pub(super) bindings: super::bindings::BindingCollections,
    pub(super) client: Client,
    pub(super) checkpoint_objects: MongoCheckpointObjectStore,
    pub(super) sandboxes: Collection<Document>,
    pub(super) lifecycle_operations: Collection<Document>,
    pub(super) sandbox_sessions: Collection<Document>,
    pub(super) strict_inactivity_session_evidence: bool,
    pub(super) workspaces: Collection<Document>,
    pub(super) bounded_inactivity_legacy_log: bool,
    pub(super) worktrees: Collection<Document>,
    pub(super) agents: Collection<Document>,
    pub(super) agent_writes: std::sync::Arc<super::agent_write_coordination::AgentWrites>,
    pub(super) agent_control_write_coordination: bool,
    pub(crate) agent_ownership: Option<std::sync::Arc<super::agent_ownership::MongoAgentOwnership>>,
    pub(super) agent_manager_instance: Option<String>,
    pub(super) platform: std::sync::Arc<super::platform_write_coordination::PlatformState>,
    pub(super) agent_access_tools: Collection<Document>,
    pub(super) agent_access_requests: Collection<Document>,
    pub(super) agent_reuse_bindings: Collection<Document>,
    pub(super) agent_log: Collection<Document>,
    pub(super) suggestions: Collection<Document>,
    pub(super) agent_creation_provenance: Collection<Document>,
    pub(super) agent_channel_usage: Collection<Document>,
    pub(super) channel_usage_events: Collection<Document>,
    pub(super) agent_commands: Collection<Document>,
    pub(super) model_budget: Collection<Document>,
    pub(super) budget_settings: Collection<Document>,
    pub(super) agent_artifacts: Collection<Document>,
    pub(super) media_uploads: Collection<Document>,
    pub(super) sandbox_controls: Collection<Document>,
    pub(super) command_authorization_audits: Collection<Document>,
    pub(super) agent_command_orders: Collection<Document>,
    pub(super) resource_policies: Collection<Document>,
    pub(super) resource_share_commands: Collection<Document>,
    pub(super) resource_shares: Collection<Document>,
    pub(super) sandbox_events: Collection<Document>,
    pub(super) sandbox_usage: Collection<Document>,
    pub(super) sandbox_usage_events: Collection<Document>,
    pub(super) sandbox_billing: Collection<Document>,
    pub(super) sandbox_admissions: Collection<Document>,
    pub(super) checkpoints: Collection<Document>,
    pub(super) sandbox_profiles: Collection<Document>,
    #[cfg(test)]
    pub(super) sandbox_profile_revisions: Collection<Document>,
    #[cfg(test)]
    pub(super) warm_workspace_revisions: Collection<Document>,
    #[cfg(test)]
    pub(super) warm_agent_revisions: Collection<Document>,
    pub(super) warm_sandboxes: Collection<Document>,
    pub(super) native_prepared: Collection<Document>,
    pub(super) warm_runtime_version: Option<String>,
    pub(super) managed_runtime_version: Option<String>,
    pub(crate) default_pool_holds: std::sync::Arc<crate::warm_pools::DefaultPoolHolds>,
    pub(crate) runtime_image_origin_enabled: bool,
    pub(crate) managed_machine_updates: crate::config::ManagedMachineUpdates,
    pub(crate) desktop_verified_runtime_image: Option<String>,
    pub(super) managed_machine_runtime_version: Option<String>,
    /// The process's released-default authority. When set it supersedes the
    /// two fixed pins above, which remain only for store fixtures.
    pub(super) released: Option<std::sync::Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
    pub(super) released_lanes: Option<super::released_runtimes::MongoReleasedRuntimes>,
    pub(super) warm_home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub(crate) warmup_changed: std::sync::Arc<tokio::sync::Notify>,
    pub(crate) native_prepared_changed: std::sync::Arc<tokio::sync::Notify>,
    // Coalesced process-local hints; durable discovery owns recovery and authority.
    pub(crate) sandbox_admitted: std::sync::Arc<tokio::sync::Notify>,
    pub(crate) access_admitted: std::sync::Arc<tokio::sync::Notify>,
    pub(crate) agent_admitted: std::sync::Arc<tokio::sync::Notify>,
    // Broadcast to already leased parent waiters without consuming scheduler hints.
    pub(crate) agent_parent_ready: std::sync::Arc<tokio::sync::Notify>,
}

impl MongoSandboxStore {
    pub(crate) fn with_managed_runtime_version(mut self, version: String) -> Self {
        self.managed_runtime_version = Some(version);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_machine_runtime_version(mut self, version: Option<String>) -> Self {
        self.managed_machine_runtime_version = version;
        self
    }

    pub(crate) fn with_released_runtime(
        mut self,
        released: std::sync::Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    ) -> Self {
        self.released = Some(released);
        self
    }

    /// The current release of both lanes, captured once.
    pub(crate) fn released_runtime(&self) -> layer3_adapter::ReleasedRuntime {
        match &self.released {
            Some(released) => released.snapshot(),
            None => layer3_adapter::ReleasedRuntime {
                revision: 0,
                agent: self.managed_runtime_version.clone().unwrap_or_default(),
                machine: self.managed_machine_runtime_version.clone(),
                lane_revisions: None,
            },
        }
    }

    /// The current agent-lane release for new managed work.
    pub(crate) fn managed_agent_release(&self) -> Option<String> {
        let release = match &self.released {
            Some(released) => Some(released.snapshot().agent),
            None => self.managed_runtime_version.clone(),
        };
        release.filter(|image| !self.default_pool_holds.image(image))
    }

    pub(super) async fn managed_resume_destination(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
        session: &mut ClientSession,
    ) -> Result<Option<String>, SandboxStoreError> {
        use layer2_core::sandbox::store::SandboxStore;
        if sandbox.status != layer2_core::sandbox::SandboxStatus::Paused
            || sandbox
                .runtime_update_request()
                .is_some_and(|request| request.completed_at.is_none())
        {
            return Ok(None);
        }
        let release = self.released_runtime();
        let Some((lane, version)) = self.managed_runtime_for_release(sandbox, &release) else {
            return Ok(None);
        };
        self.fence_released_admission(&release, lane, session)
            .await?;
        Ok(Some(version.into()))
    }

    pub(crate) fn with_agent_control_write_coordination(mut self, enabled: bool) -> Self {
        self.agent_control_write_coordination = enabled;
        self
    }

    pub(crate) fn with_bounded_inactivity_legacy_log(mut self, enabled: bool) -> Self {
        self.bounded_inactivity_legacy_log = enabled;
        self
    }

    pub(crate) fn with_strict_inactivity_session_evidence(mut self, enabled: bool) -> Self {
        self.strict_inactivity_session_evidence = enabled;
        self
    }

    #[cfg(test)]
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, StoreError> {
        Self::open_with_storage(client, name, super::checkpoint_storage::test_storage(name)).await
    }

    #[cfg(test)]
    pub(super) async fn open_with_storage(
        client: &Client,
        name: &str,
        objects: std::sync::Arc<dyn super::CheckpointBlobStore>,
    ) -> Result<Self, StoreError> {
        Self::open_with_schema(
            client,
            name,
            objects,
            &mut super::schema::StartupObserver::new(Default::default()),
            Default::default(),
        )
        .await
    }

    pub(super) async fn open_with_schema(
        client: &Client,
        name: &str,
        objects: std::sync::Arc<dyn super::CheckpointBlobStore>,
        observer: &mut super::schema::StartupObserver,
        initial_settings: super::settings_defaults::InitialSettings,
    ) -> Result<Self, StoreError> {
        let database = client.database(name);
        require_transaction_topology(&database).await?;
        initialize_collections(&database, SANDBOX_COLLECTIONS).await?;
        let checkpoint_objects =
            MongoCheckpointObjectStore::open_with_storage(client, name, objects).await?;
        let bindings = super::bindings::BindingCollections::open(&database, observer).await?;
        let store = Self::handles(client, name, checkpoint_objects, bindings, initial_settings);
        super::budget::initialize_settings(&store, initial_settings).await?;
        super::sandboxes::ensure_indexes(&store.sandboxes).await?;
        super::sandbox_sessions::ensure_indexes(&store.sandbox_sessions).await?;
        super::sandbox_usage::ensure_indexes(&store).await?;
        super::sandbox_billing::ensure_indexes(&store.sandbox_billing).await?;
        super::workspaces::ensure_indexes(&store.workspaces).await?;
        super::worktrees::ensure_indexes(&store.worktrees).await?;
        super::agents::ensure_indexes(&store.agents).await?;
        super::agent_access_requests::ensure_indexes(&store).await?;
        store
            .agent_access_tools
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {"agent_id": 1, "record.conversation": 1})
                    .build(),
            )
            .await
            .map_err(|error| unavailable("Agent access tool index", error))?;
        super::trigger_first_messages::ensure_indexes(&store).await?;
        super::agent_log::ensure_indexes(&store.agent_log).await?;
        super::agent_task_status::ensure_indexes(&store.agent_log).await?;
        super::suggestions::ensure_indexes(&store.suggestions).await?;
        super::agent_channel_usage::ensure_indexes(&store.channel_usage_events).await?;
        super::agent_commands::ensure_indexes(&store.agent_commands).await?;
        super::sandbox_controls::ensure_indexes(&store.sandbox_controls).await?;
        super::agent_commands::audit::ensure_indexes(&store.command_authorization_audits).await?;
        super::resource_access::ensure_indexes(&store).await?;
        super::resource_access::ensure_artifact_indexes(&store.agent_artifacts).await?;
        super::sandbox_events::ensure_indexes(&store.sandbox_events).await?;
        super::checkpoints::ensure_indexes(&store.checkpoints).await?;
        super::warm_sandboxes::ensure_indexes(&store.warm_sandboxes).await?;
        super::native_prepared::ensure_indexes(&store.native_prepared).await?;
        tracing::info!(database = name, "sandbox-model Mongo store ready");
        Ok(store.with_title_effect_custody(false))
    }
}

/// Bound reconciliation without cancelling a Mongo driver future mid-command.
pub(super) async fn wait_for_reconciliation<F, T>(
    context: &'static str,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    wait_for_reconciliation_with_deadline(context, RECONCILIATION_TIMEOUT, future).await
}

async fn wait_for_reconciliation_with_deadline<F, T>(
    context: &'static str,
    deadline: Duration,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let permit = RECONCILIATION_SLOTS.try_acquire().map_err(|_| {
        format!(
            "{context} cannot start while {MAX_PENDING_RECONCILIATIONS} prior reconciliations remain pending"
        )
    })?;
    let mut task = tokio::spawn(crate::telemetry::agent_output::inherit_commands(
        async move {
            let _permit = permit;
            future.await
        },
    ));
    match tokio::time::timeout(deadline, &mut task).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => {
            tracing::error!(
                context,
                panicked = error.is_panic(),
                "reconciliation task terminated"
            );
            Err(format!("{context} task terminated before completion"))
        }
        Err(_) => {
            tracing::warn!(
                context,
                deadline_seconds = deadline.as_secs(),
                "reconciliation exceeded its deadline and will finish in the background"
            );
            tokio::spawn(async move {
                match task.await {
                    Ok(_) => tracing::info!(context, "timed-out reconciliation finished"),
                    Err(error) => tracing::error!(
                        context,
                        panicked = error.is_panic(),
                        "timed-out reconciliation task terminated"
                    ),
                }
            });
            Err(format!(
                "{context} exceeded its {} second deadline",
                deadline.as_secs()
            ))
        }
    }
}

pub(super) fn unavailable(context: &str, error: Error) -> StoreError {
    StoreError::Unavailable(format!("{context}: {error}"))
}

#[cfg(test)]
#[path = "sandbox_mongo_tests.rs"]
mod tests;

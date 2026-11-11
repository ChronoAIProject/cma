//! Shared MongoDB boundary for the fresh sandbox-model schema.

use std::future::Future;
use std::time::Duration;

use layer2_core::StoreError;
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};
use mongodb::options::{ReadConcern, WriteConcern};
use mongodb::{Client, ClientSession, Collection, Database};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::Semaphore;

use super::MongoCheckpointObjectStore;
use super::{durability::majority_collection, schema::initialize_collections};

pub use super::profile_mongo::MongoProfileStore;
pub(super) use super::schema::DOCUMENT_SCHEMA_VERSION;
pub(super) const TRANSACTION_ATTEMPTS: usize = 5;
const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;
const SNAPSHOT_READ_MIN_WIRE_VERSION: i64 = 13;
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

const SANDBOX_COLLECTIONS: &[&str] = &[
    super::settings::SETTINGS,
    super::budget::COLLECTION,
    "sandbox_retained_availability",
    SANDBOXES,
    SANDBOX_SESSIONS,
    WORKSPACES,
    WORKTREES,
    AGENTS,
    super::agent_access_tools::COLLECTION,
    super::agent_access_requests::COLLECTION,
    AGENT_REUSE_BINDINGS,
    super::trigger_first_messages::COLLECTION,
    AGENT_LOG,
    "agent_suggestions",
    "agent_channel_usage",
    "channel_usage_events",
    AGENT_COMMANDS,
    AGENT_ARTIFACTS,
    super::media_uploads::COLLECTION,
    SANDBOX_CONTROLS,
    AGENT_COMMAND_ORDERS,
    RESOURCE_POLICIES,
    RESOURCE_SHARES,
    RESOURCE_SHARE_COMMANDS,
    SANDBOX_EVENTS,
    super::sandbox_usage::STATES,
    super::sandbox_usage::EVENTS,
    SANDBOX_BILLING,
    SANDBOX_ADMISSIONS,
    CHECKPOINTS,
    WARM_SANDBOXES,
    super::native_prepared::COLLECTION,
];

/// One adapter implements every sandbox-model port so cross-port operations
/// share a client session and therefore a real Mongo transaction.
#[derive(Clone)]
pub struct MongoSandboxStore {
    pub(super) global_sandbox_limit: u16,
    #[cfg(test)]
    pub(super) request_budget_clock: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    pub(super) bounded_approval_reads: bool,
    pub(super) bounded_close_retries: bool,
    pub(super) fleet_log_activation: std::sync::Arc<super::agent_log_integrity::fleet::Activation>,
    pub(super) bounded_admission_agents: std::collections::BTreeSet<layer2_core::AgentId>,
    pub(super) bounded_title_reads: bool,
    pub(super) bounded_response_reads: bool,
    pub(crate) writer_policy:
        std::sync::Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    pub(super) bindings: super::bindings::BindingCollections,
    pub(super) client: Client,
    pub(super) checkpoint_objects: MongoCheckpointObjectStore,
    pub(super) sandboxes: Collection<Document>,
    pub(super) sandbox_sessions: Collection<Document>,
    pub(super) workspaces: Collection<Document>,
    pub(super) worktrees: Collection<Document>,
    pub(super) agents: Collection<Document>,
    pub(super) agent_writes: std::sync::Arc<super::agent_write_coordination::AgentWrites>,
    pub(super) platform_writes: std::sync::Arc<super::platform_write_coordination::PlatformWrites>,
    pub(super) platform_stop_cache:
        std::sync::Arc<std::sync::Mutex<super::budget::PlatformStopCache>>,
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
    pub(super) managed_machine_runtime_version: Option<String>,
    pub(super) warm_home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub(crate) warmup_changed: std::sync::Arc<tokio::sync::Notify>,
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

    pub(crate) fn with_machine_runtime_version(mut self, version: Option<String>) -> Self {
        self.managed_machine_runtime_version = version;
        self
    }

    #[cfg(test)]
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, StoreError> {
        Self::open_with_storage(client, name, super::checkpoint_storage::test_storage(name)).await
    }

    pub(super) async fn open_with_storage(
        client: &Client,
        name: &str,
        objects: std::sync::Arc<dyn super::CheckpointBlobStore>,
    ) -> Result<Self, StoreError> {
        let database = client.database(name);
        require_transaction_topology(&database).await?;
        initialize_collections(&database, SANDBOX_COLLECTIONS).await?;
        let checkpoint_objects =
            MongoCheckpointObjectStore::open_with_storage(client, name, objects).await?;
        let store = Self {
            global_sandbox_limit: 0,
            #[cfg(test)]
            request_budget_clock: None,
            bounded_admission_agents: Default::default(),
            bounded_approval_reads: false,
            bounded_close_retries: false,
            fleet_log_activation: Default::default(),
            bounded_title_reads: false,
            bounded_response_reads: false,
            writer_policy: Default::default(),
            bindings: super::bindings::BindingCollections::open(&database).await?,
            client: client.clone(),
            checkpoint_objects,
            sandboxes: majority_collection(&database, SANDBOXES),
            sandbox_sessions: majority_collection(&database, SANDBOX_SESSIONS),
            workspaces: majority_collection(&database, WORKSPACES),
            worktrees: majority_collection(&database, WORKTREES),
            agents: majority_collection(&database, AGENTS),
            agent_writes: Default::default(),
            platform_writes: Default::default(),
            platform_stop_cache: Default::default(),
            agent_access_tools: majority_collection(
                &database,
                super::agent_access_tools::COLLECTION,
            ),
            agent_access_requests: majority_collection(
                &database,
                super::agent_access_requests::COLLECTION,
            ),
            agent_reuse_bindings: majority_collection(&database, AGENT_REUSE_BINDINGS),
            agent_log: majority_collection(&database, AGENT_LOG),
            suggestions: majority_collection(&database, "agent_suggestions"),
            agent_creation_provenance: majority_collection(&database, "agent_creation_provenance"),
            agent_channel_usage: majority_collection(&database, "agent_channel_usage"),
            channel_usage_events: majority_collection(&database, "channel_usage_events"),
            agent_commands: majority_collection(&database, AGENT_COMMANDS),
            model_budget: majority_collection(&database, super::budget::COLLECTION),
            budget_settings: majority_collection(&database, "platform_settings"),
            agent_artifacts: majority_collection(&database, AGENT_ARTIFACTS),
            media_uploads: majority_collection(&database, super::media_uploads::COLLECTION),
            sandbox_controls: majority_collection(&database, SANDBOX_CONTROLS),
            command_authorization_audits: majority_collection(
                &database,
                COMMAND_AUTHORIZATION_AUDITS,
            ),
            agent_command_orders: majority_collection(&database, AGENT_COMMAND_ORDERS),
            resource_policies: majority_collection(&database, RESOURCE_POLICIES),
            resource_share_commands: majority_collection(&database, RESOURCE_SHARE_COMMANDS),
            resource_shares: majority_collection(&database, RESOURCE_SHARES),
            sandbox_events: majority_collection(&database, SANDBOX_EVENTS),
            sandbox_usage: majority_collection(&database, super::sandbox_usage::STATES),
            sandbox_usage_events: majority_collection(&database, super::sandbox_usage::EVENTS),
            sandbox_billing: majority_collection(&database, SANDBOX_BILLING),
            sandbox_admissions: majority_collection(&database, SANDBOX_ADMISSIONS),
            checkpoints: majority_collection(&database, CHECKPOINTS),
            sandbox_profiles: majority_collection(&database, SANDBOX_PROFILES),
            #[cfg(test)]
            sandbox_profile_revisions: majority_collection(&database, SANDBOX_PROFILE_REVISIONS),
            #[cfg(test)]
            warm_workspace_revisions: majority_collection(&database, WORKSPACE_PROFILE_REVISIONS),
            #[cfg(test)]
            warm_agent_revisions: majority_collection(&database, AGENT_PROFILE_REVISIONS),
            warm_sandboxes: majority_collection(&database, WARM_SANDBOXES),
            native_prepared: majority_collection(&database, super::native_prepared::COLLECTION),
            warm_runtime_version: None,
            managed_runtime_version: None,
            managed_machine_runtime_version: None,
            warm_home_policy: None,
            warmup_changed: std::sync::Arc::new(tokio::sync::Notify::new()),
            sandbox_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            access_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            agent_admitted: std::sync::Arc::new(tokio::sync::Notify::new()),
            agent_parent_ready: std::sync::Arc::new(tokio::sync::Notify::new()),
        };
        super::budget::initialize_settings(&store).await?;
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
        Ok(store)
    }
}

pub(super) async fn require_transaction_topology(database: &Database) -> Result<(), StoreError> {
    let reply = database
        .run_command(doc! { "hello": 1_i32 })
        .await
        .map_err(|error| unavailable("inspect Mongo transaction topology", error))?;
    validate_transaction_server(&reply)
}

fn validate_transaction_server(reply: &Document) -> Result<(), StoreError> {
    if reply.get_str("setName").is_err()
        && !matches!(reply.get_str("msg"), Ok(message) if message == "isdbgrid")
    {
        return Err(StoreError::Unavailable(
            "sandbox-model transactions require a replica set or mongos".into(),
        ));
    }
    let max_wire_version = match reply.get("maxWireVersion") {
        Some(Bson::Int32(version)) => i64::from(*version),
        Some(Bson::Int64(version)) => *version,
        _ => {
            return Err(StoreError::Unavailable(
                "MongoDB did not report a usable maxWireVersion".into(),
            ));
        }
    };
    if max_wire_version < SNAPSHOT_READ_MIN_WIRE_VERSION {
        return Err(StoreError::Unavailable(format!(
            "snapshot reads require MongoDB 5.0 or later (maxWireVersion >= {SNAPSHOT_READ_MIN_WIRE_VERSION}); server reported {max_wire_version}"
        )));
    }
    Ok(())
}

pub(super) fn record_bson<T: Serialize>(record: &T) -> Result<Bson, String> {
    mongodb::bson::serialize_to_bson(record).map_err(|error| error.to_string())
}

pub(super) fn read_record<T: DeserializeOwned>(document: &Document) -> Result<T, String> {
    require_version(document)?;
    let record = document
        .get("record")
        .cloned()
        .ok_or_else(|| "stored document has no aggregate record".to_string())?;
    mongodb::bson::deserialize_from_bson(record).map_err(|error| error.to_string())
}

pub(super) fn require_version(document: &Document) -> Result<(), String> {
    match document.get("document_schema_version") {
        Some(Bson::Int32(version)) if i64::from(*version) == DOCUMENT_SCHEMA_VERSION => Ok(()),
        Some(Bson::Int64(version)) if *version == DOCUMENT_SCHEMA_VERSION => Ok(()),
        Some(version) => Err(format!("unsupported document schema version {version}")),
        None => Err("stored document has no schema version".into()),
    }
}

pub(super) fn signed(value: u64, field: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds the BSON integer range"))
}

pub(super) fn unsigned(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} must not be negative"))
}

pub(super) fn nullable(value: Option<&String>) -> Bson {
    value.cloned().map(Bson::String).unwrap_or(Bson::Null)
}

pub(super) fn string_field_matches(document: &Document, field: &str, expected: &str) -> bool {
    matches!(document.get_str(field), Ok(actual) if actual == expected)
}

pub(super) fn i64_field_matches(document: &Document, field: &str, expected: i64) -> bool {
    matches!(document.get_i64(field), Ok(actual) if actual == expected)
}

pub(super) fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

pub(super) fn is_transient(error: &Error) -> bool {
    error.contains_label(TRANSIENT_TRANSACTION_ERROR)
}

pub(super) enum CommitOutcome {
    Committed,
    Retry(Error),
    Unknown(Error),
    Failed(Error),
}

pub(super) async fn start_transaction(session: &mut ClientSession) -> Result<(), Error> {
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await
}

pub(super) async fn commit_transaction(session: &mut ClientSession) -> CommitOutcome {
    for attempt in 0..COMMIT_ATTEMPTS {
        match session.commit_transaction().await {
            Ok(()) => return CommitOutcome::Committed,
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                if attempt + 1 == COMMIT_ATTEMPTS {
                    return CommitOutcome::Unknown(error);
                }
                tokio::task::yield_now().await;
            }
            Err(error) if is_transient(&error) => return CommitOutcome::Retry(error),
            Err(error) => return CommitOutcome::Failed(error),
        }
    }
    unreachable!("the bounded commit loop always returns")
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
    let mut task = tokio::spawn(async move {
        let _permit = permit;
        future.await
    });
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

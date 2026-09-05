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

use super::{durability::majority_collection, schema::initialize_collections};

pub(super) use super::schema::DOCUMENT_SCHEMA_VERSION;
pub(super) const TRANSACTION_ATTEMPTS: usize = 5;
const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;
const SNAPSHOT_READ_MIN_WIRE_VERSION: i64 = 13;
const RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PENDING_RECONCILIATIONS: usize = 16;
static RECONCILIATION_SLOTS: Semaphore = Semaphore::const_new(MAX_PENDING_RECONCILIATIONS);

pub(super) const SANDBOXES: &str = "sandboxes";
pub(super) const WORKSPACES: &str = "workspaces";
pub(super) const WORKTREES: &str = "worktrees";
pub(super) const AGENTS: &str = "agents";
pub(super) const AGENT_LOG: &str = "agent_log";
pub(super) const SANDBOX_EVENTS: &str = "sandbox_events";
pub(super) const SANDBOX_ADMISSIONS: &str = "sandbox_admissions";
pub(super) const CHECKPOINTS: &str = "checkpoints";
pub(super) const AGENT_PROFILES: &str = "agent_profiles";
pub(super) const AGENT_PROFILE_REVISIONS: &str = "agent_profile_revisions";
pub(super) const WORKSPACE_PROFILES: &str = "workspace_profiles";
pub(super) const WORKSPACE_PROFILE_REVISIONS: &str = "workspace_profile_revisions";
pub(super) const SANDBOX_PROFILES: &str = "sandbox_profiles";
pub(super) const SANDBOX_PROFILE_REVISIONS: &str = "sandbox_profile_revisions";

const SANDBOX_COLLECTIONS: &[&str] = &[
    SANDBOXES,
    WORKSPACES,
    WORKTREES,
    AGENTS,
    AGENT_LOG,
    SANDBOX_EVENTS,
    SANDBOX_ADMISSIONS,
    CHECKPOINTS,
];
const PROFILE_COLLECTIONS: &[&str] = &[
    AGENT_PROFILES,
    AGENT_PROFILE_REVISIONS,
    WORKSPACE_PROFILES,
    WORKSPACE_PROFILE_REVISIONS,
    SANDBOX_PROFILES,
    SANDBOX_PROFILE_REVISIONS,
];

/// One adapter implements every sandbox-model port so cross-port operations
/// share a client session and therefore a real Mongo transaction.
#[derive(Clone)]
pub struct MongoSandboxStore {
    pub(super) client: Client,
    pub(super) sandboxes: Collection<Document>,
    pub(super) workspaces: Collection<Document>,
    pub(super) worktrees: Collection<Document>,
    pub(super) agents: Collection<Document>,
    pub(super) agent_log: Collection<Document>,
    pub(super) sandbox_events: Collection<Document>,
    pub(super) sandbox_admissions: Collection<Document>,
    pub(super) checkpoints: Collection<Document>,
}

impl MongoSandboxStore {
    pub(super) async fn open(client: &Client, name: &str) -> Result<Self, StoreError> {
        let database = client.database(name);
        require_transaction_topology(&database).await?;
        initialize_collections(&database, SANDBOX_COLLECTIONS).await?;
        let store = Self {
            client: client.clone(),
            sandboxes: majority_collection(&database, SANDBOXES),
            workspaces: majority_collection(&database, WORKSPACES),
            worktrees: majority_collection(&database, WORKTREES),
            agents: majority_collection(&database, AGENTS),
            agent_log: majority_collection(&database, AGENT_LOG),
            sandbox_events: majority_collection(&database, SANDBOX_EVENTS),
            sandbox_admissions: majority_collection(&database, SANDBOX_ADMISSIONS),
            checkpoints: majority_collection(&database, CHECKPOINTS),
        };
        super::sandboxes::ensure_indexes(&store.sandboxes).await?;
        super::workspaces::ensure_indexes(&store.workspaces).await?;
        super::worktrees::ensure_indexes(&store.worktrees).await?;
        super::agents::ensure_indexes(&store.agents).await?;
        super::agent_log::ensure_indexes(&store.agent_log).await?;
        super::sandbox_events::ensure_indexes(&store.sandbox_events).await?;
        super::checkpoints::ensure_indexes(&store.checkpoints).await?;
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

/// Profile drafts and revisions are separate from runtime aggregates, but use
/// the same Mongo client so publication can resolve references transactionally.
#[derive(Clone)]
pub struct MongoProfileStore {
    pub(super) client: Client,
    pub(super) agent_profiles: Collection<Document>,
    pub(super) agent_revisions: Collection<Document>,
    pub(super) workspace_profiles: Collection<Document>,
    pub(super) workspace_revisions: Collection<Document>,
    pub(super) sandbox_profiles: Collection<Document>,
    pub(super) sandbox_revisions: Collection<Document>,
}

impl MongoProfileStore {
    pub(super) async fn open(client: &Client, name: &str) -> Result<Self, StoreError> {
        let database = client.database(name);
        initialize_collections(&database, PROFILE_COLLECTIONS).await?;
        let store = Self {
            client: client.clone(),
            agent_profiles: majority_collection(&database, AGENT_PROFILES),
            agent_revisions: majority_collection(&database, AGENT_PROFILE_REVISIONS),
            workspace_profiles: majority_collection(&database, WORKSPACE_PROFILES),
            workspace_revisions: majority_collection(&database, WORKSPACE_PROFILE_REVISIONS),
            sandbox_profiles: majority_collection(&database, SANDBOX_PROFILES),
            sandbox_revisions: majority_collection(&database, SANDBOX_PROFILE_REVISIONS),
        };
        super::profiles::ensure_indexes(&store).await?;
        tracing::info!(database = name, "sandbox-profile Mongo store ready");
        Ok(store)
    }
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
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn unknown_versions_include_missing_wrong_and_non_integer_values() {
        assert!(require_version(&doc! { "document_schema_version": 1_i64 }).is_ok());
        assert!(require_version(&doc! { "document_schema_version": 1_i32 }).is_ok());
        assert!(require_version(&doc! { "document_schema_version": 2_i64 }).is_err());
        assert!(require_version(&doc! { "document_schema_version": 1.0 }).is_err());
        assert!(require_version(&Document::new()).is_err());
    }

    #[test]
    fn transaction_server_requires_topology_and_snapshot_support() {
        assert!(
            validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 13_i32 })
                .is_ok()
        );
        assert!(
            validate_transaction_server(&doc! { "msg": "isdbgrid", "maxWireVersion": 25_i64 })
                .is_ok()
        );

        let standalone = validate_transaction_server(&doc! { "maxWireVersion": 25_i32 })
            .unwrap_err()
            .to_string();
        assert!(standalone.contains("replica set or mongos"));
        let old = validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 12_i32 })
            .unwrap_err()
            .to_string();
        assert!(old.contains("MongoDB 5.0 or later"));
        let missing = validate_transaction_server(&doc! { "setName": "rs0" })
            .unwrap_err()
            .to_string();
        assert!(missing.contains("maxWireVersion"));
    }

    #[tokio::test(start_paused = true)]
    async fn reconciliation_deadline_detaches_instead_of_cancelling_driver_work() {
        let (release, blocked) = tokio::sync::oneshot::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let completed = Arc::clone(&finished);
        let result = wait_for_reconciliation_with_deadline(
            "test reconciliation",
            Duration::from_secs(1),
            async move {
                let _ = blocked.await;
                completed.store(true, Ordering::SeqCst);
            },
        )
        .await;

        assert_eq!(
            result,
            Err("test reconciliation exceeded its 1 second deadline".into())
        );
        assert!(!finished.load(Ordering::SeqCst));
        release.send(()).unwrap();
        tokio::task::yield_now().await;
        assert!(finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn caller_cancellation_detaches_an_active_reconciliation() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
        let caller = tokio::spawn(wait_for_reconciliation("cancelled caller", async move {
            started_tx.send(()).unwrap();
            let _ = release_rx.await;
            finished_tx.send(()).unwrap();
        }));

        started_rx.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();
        finished_rx.await.unwrap();
    }
}

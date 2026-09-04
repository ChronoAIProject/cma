//! Shared MongoDB boundary for the fresh sandbox-model schema.

use std::collections::HashSet;

use layer2_core::StoreError;
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};
use mongodb::options::{ReadConcern, WriteConcern};
use mongodb::{Client, ClientSession, Collection, Database};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::durability::majority_collection;

pub(super) const DOCUMENT_SCHEMA_VERSION: i64 = 1;
pub(super) const TRANSACTION_ATTEMPTS: usize = 5;
const COMMIT_ATTEMPTS: usize = 5;
const DUPLICATE_KEY: i32 = 11_000;

pub(super) const SANDBOXES: &str = "sandboxes";
pub(super) const WORKSPACES: &str = "workspaces";
pub(super) const WORKTREES: &str = "worktrees";
pub(super) const AGENTS: &str = "agents";
pub(super) const AGENT_LOG: &str = "agent_log";
pub(super) const SANDBOX_EVENTS: &str = "sandbox_events";
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

async fn require_transaction_topology(database: &Database) -> Result<(), StoreError> {
    let reply = database
        .run_command(doc! { "hello": 1_i32 })
        .await
        .map_err(|error| unavailable("inspect Mongo transaction topology", error))?;
    if reply.get_str("setName").is_ok()
        || matches!(reply.get_str("msg"), Ok(message) if message == "isdbgrid")
    {
        Ok(())
    } else {
        Err(StoreError::Unavailable(
            "sandbox-model transactions require a replica set or mongos".into(),
        ))
    }
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

async fn initialize_collections(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    let existing: HashSet<_> = database
        .list_collection_names()
        .await
        .map_err(|error| unavailable("list schema-v1 collections", error))?
        .into_iter()
        .collect();
    for name in collections.iter().filter(|name| !existing.contains(**name)) {
        if let Err(error) = database.create_collection(*name).await {
            let appeared = database
                .list_collection_names()
                .await
                .map_err(|list_error| unavailable("recheck schema-v1 collection", list_error))?
                .iter()
                .any(|candidate| candidate == name);
            if !appeared {
                return Err(unavailable(
                    &format!("create schema-v1 collection {name}"),
                    error,
                ));
            }
        }
    }
    refuse_unknown_versions(database, collections).await
}

async fn refuse_unknown_versions(
    database: &Database,
    collections: &[&str],
) -> Result<(), StoreError> {
    for name in collections {
        let unsupported = database
            .collection::<Document>(name)
            .find_one(unknown_version_filter())
            .projection(doc! { "_id": 1, "document_schema_version": 1 })
            .await
            .map_err(|error| unavailable(&format!("inspect {name} schema version"), error))?;
        if let Some(document) = unsupported {
            let id = document.get("_id").unwrap_or(&Bson::Null);
            let version = document
                .get("document_schema_version")
                .unwrap_or(&Bson::Null);
            return Err(StoreError::Malformed(format!(
                "collection {name} contains unsupported document {id} with schema version {version}"
            )));
        }
    }
    Ok(())
}

pub(super) fn unknown_version_filter() -> Document {
    doc! {
        "$or": [
            { "document_schema_version": { "$ne": DOCUMENT_SCHEMA_VERSION } },
            {
                "document_schema_version": {
                    "$not": { "$type": ["int", "long"] }
                }
            },
        ]
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

pub(super) fn unavailable(context: &str, error: Error) -> StoreError {
    StoreError::Unavailable(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_versions_include_missing_wrong_and_non_integer_values() {
        assert_eq!(unknown_version_filter()["$or"].as_array().unwrap().len(), 2);
        assert!(require_version(&doc! { "document_schema_version": 1_i64 }).is_ok());
        assert!(require_version(&doc! { "document_schema_version": 1_i32 }).is_ok());
        assert!(require_version(&doc! { "document_schema_version": 2_i64 }).is_err());
        assert!(require_version(&doc! { "document_schema_version": 1.0 }).is_err());
        assert!(require_version(&Document::new()).is_err());
    }
}

//! Trigger drafts, immutable revisions, and fenced Launch aggregates in MongoDB.

use futures_util::TryStreamExt as _;
use layer2_core::{
    CapabilityRevocationState, ConnectionRevocationLease, ConnectionRevocationRecord,
    ConnectionRevocationStore, Launch, LaunchEvent, LaunchId, LaunchQuery, LaunchState,
    LaunchStore, PublicTriggerId, PublishedRevision, SubjectId, TriggerDraft, TriggerId,
    TriggerPage, TriggerPageRequest, TriggerRevision, TriggerRevisionId, TriggerStore,
    TriggerStoreError, WorkerLease, LAUNCH_EVENT_TAIL_LIMIT, TRIGGER_CAPABILITY_STATES,
    TRIGGER_WORKER_STATES,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::error::{
    Error, ErrorKind, WriteFailure, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
};
use mongodb::options::{IndexOptions, ReturnDocument};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;

use super::durability::majority_collection;

const TRIGGER_DRAFTS: &str = "trigger_drafts";
const TRIGGER_REVISIONS: &str = "trigger_revisions";
const TRIGGER_LAUNCHES: &str = "trigger_launches";
const TRIGGER_LAUNCH_EVENTS: &str = "trigger_launch_events";
const TRIGGER_CONNECTION_REVOCATIONS: &str = "trigger_connection_revocations";
const DUPLICATE_KEY: i32 = 11_000;
const CONNECTION_REVOCATION_CAS_RETRIES: usize = 5;
const TRIGGER_PUBLICATION_TRANSACTION_ATTEMPTS: usize = 5;
const TRIGGER_PUBLICATION_COMMIT_ATTEMPTS: usize = 5;
const LAUNCH_TRANSACTION_COMMIT_ATTEMPTS: usize = 5;
const LAUNCH_TRANSACTION_COMMIT_DEADLINE: Duration = Duration::from_secs(5);
const LAUNCH_TRANSACTION_COMMIT_INITIAL_BACKOFF: Duration = Duration::from_millis(25);
const LAUNCH_TRANSACTION_COMMIT_MAX_BACKOFF: Duration = Duration::from_millis(400);
const TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION: i64 = 1;
const TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DraftDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    document_schema_version: i64,
    owner: String,
    version: i64,
    updated_at: String,
    draft: TriggerDraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RevisionDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    document_schema_version: i64,
    public_id: String,
    owner: String,
    published_at: String,
    revision: TriggerRevision,
}

#[derive(Debug, Deserialize)]
struct DraftVersionProjection {
    #[serde(default)]
    document_schema_version: i64,
    version: i64,
}

enum TransactionCommitOutcome {
    Committed,
    RetryTransaction(Error),
    Unknown(Error),
}

enum LaunchCommitAttempt {
    Committed,
    Unknown(Error),
    Failed(Error),
}

#[derive(Debug)]
enum LaunchCommitFailure {
    Unknown(Error),
    Deadline(Option<Error>),
    Failed(Error),
}

#[async_trait::async_trait]
trait LaunchTransactionCommitter {
    async fn commit_once(&mut self) -> LaunchCommitAttempt;
}

#[async_trait::async_trait]
impl LaunchTransactionCommitter for mongodb::ClientSession {
    async fn commit_once(&mut self) -> LaunchCommitAttempt {
        match self.commit_transaction().await {
            Ok(()) => LaunchCommitAttempt::Committed,
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                LaunchCommitAttempt::Unknown(error)
            }
            Err(error) => LaunchCommitAttempt::Failed(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LaunchDoc {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    revision_id: String,
    current: bool,
    state: LaunchState,
    created_at: String,
    updated_at: String,
    revision_number: i64,
    fencing_counter: i64,
    #[serde(default)]
    lease_expires_at: Option<String>,
    /// `None` identifies a legacy document whose embedded stream has not yet
    /// been copied to the canonical event collection.
    #[serde(default)]
    event_high_water: Option<i64>,
    #[serde(deserialize_with = "deserialize_launch")]
    launch: Launch,
}

fn deserialize_launch<'de, D>(deserializer: D) -> Result<Launch, D::Error>
where
    D: Deserializer<'de>,
{
    let mut document = Document::deserialize(deserializer)?;
    match document.get_mut("runtime_admission") {
        None | Some(Bson::Null) => {}
        Some(Bson::Document(runtime)) => match runtime.get("kernel") {
            None => {
                runtime.insert("kernel", crate::trigger::TRIGGER_KERNEL_ID);
            }
            Some(Bson::String(kernel)) if kernel == crate::trigger::TRIGGER_KERNEL_ID => {}
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "stored Launch runtime admission uses an unsupported kernel",
                ));
            }
        },
        Some(_) => {
            return Err(serde::de::Error::custom(
                "stored Launch runtime admission is not a document",
            ));
        }
    }
    mongodb::bson::deserialize_from_document(document).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LaunchEventDoc {
    #[serde(rename = "_id")]
    id: String,
    launch_id: String,
    sequence: i64,
    correlation_id: String,
    event: LaunchEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConnectionRevocationIdDoc {
    owner: String,
    service_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionRevocationDoc {
    #[serde(rename = "_id")]
    id: ConnectionRevocationIdDoc,
    owner: String,
    service_id: String,
    revision: i64,
    #[serde(default)]
    lease_expires_at: Option<String>,
    record: ConnectionRevocationRecord,
}

#[derive(Clone)]
pub struct MongoTriggerStore {
    client: Client,
    drafts: Collection<DraftDoc>,
    revisions: Collection<RevisionDoc>,
    launches: Collection<LaunchDoc>,
    launch_events: Collection<LaunchEventDoc>,
    connection_revocations: Collection<ConnectionRevocationDoc>,
}

impl MongoTriggerStore {
    pub(super) async fn open(
        client: &Client,
        database_name: &str,
    ) -> Result<Self, TriggerStoreError> {
        let database = client.database(database_name);
        let store = Self {
            client: client.clone(),
            drafts: majority_collection(&database, TRIGGER_DRAFTS),
            revisions: majority_collection(&database, TRIGGER_REVISIONS),
            launches: majority_collection(&database, TRIGGER_LAUNCHES),
            launch_events: majority_collection(&database, TRIGGER_LAUNCH_EVENTS),
            connection_revocations: majority_collection(&database, TRIGGER_CONNECTION_REVOCATIONS),
        };
        store.ensure_indexes().await?;
        tracing::info!("trigger store ready");
        Ok(store)
    }

    async fn migrate_draft_schema(&self, id: &str) -> Result<(), TriggerStoreError> {
        let result = self
            .drafts
            .update_one(
                exact_legacy_document_schema_filter(id),
                current_document_schema_update(TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION),
            )
            .await
            .map_err(|error| unavailable("migrate Trigger draft document schema", error))?;
        if result.modified_count == 1 {
            tracing::debug!(trigger_id = id, "migrated legacy Trigger draft schema");
        }
        Ok(())
    }

    async fn migrate_revision_schema(&self, id: &str) -> Result<(), TriggerStoreError> {
        let result = self
            .revisions
            .update_one(
                exact_legacy_document_schema_filter(id),
                current_document_schema_update(TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION),
            )
            .await
            .map_err(|error| unavailable("migrate Trigger revision document schema", error))?;
        if result.modified_count == 1 {
            tracing::debug!(
                trigger_revision_id = id,
                "migrated legacy Trigger revision schema"
            );
        }
        Ok(())
    }

    async fn migrate_draft_page(&self, docs: &[DraftDoc]) -> Result<(), TriggerStoreError> {
        for document in docs
            .iter()
            .filter(|document| document.document_schema_version == 0)
        {
            self.migrate_draft_schema(&document.id).await?;
        }
        Ok(())
    }

    async fn migrate_revision_page(&self, docs: &[RevisionDoc]) -> Result<(), TriggerStoreError> {
        for document in docs
            .iter()
            .filter(|document| document.document_schema_version == 0)
        {
            self.migrate_revision_schema(&document.id).await?;
        }
        Ok(())
    }

    async fn ensure_indexes(&self) -> Result<(), TriggerStoreError> {
        for (collection, index) in [(
            &self.drafts,
            IndexModel::builder()
                .keys(doc! { "owner": 1, "updated_at": -1, "_id": -1 })
                .options(named("trigger_drafts_by_owner"))
                .build(),
        )] {
            collection
                .create_index(index)
                .await
                .map_err(|error| unavailable("create Trigger draft index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "public_id": 1 })
                .options(unique_named("trigger_revision_public_id"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner": 1, "published_at": -1, "_id": -1 })
                .options(named("trigger_revisions_by_owner"))
                .build(),
        ] {
            self.revisions
                .create_index(index)
                .await
                .map_err(|error| unavailable("create Trigger revision index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "owner": 1, "revision_id": 1, "current": 1 })
                .options(
                    IndexOptions::builder()
                        .name("trigger_launch_current".to_string())
                        .unique(true)
                        .partial_filter_expression(doc! { "current": true })
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner": 1, "created_at": -1, "_id": -1 })
                .options(named("trigger_launches_by_owner"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner": 1, "state": 1, "created_at": -1, "_id": -1 })
                .options(named("trigger_launches_by_owner_state"))
                .build(),
            IndexModel::builder()
                .keys(doc! {
                    "owner": 1,
                    "launch.revision.public_id": 1,
                    "created_at": -1,
                    "_id": -1,
                })
                .options(named("trigger_launches_by_owner_public_id"))
                .build(),
            IndexModel::builder()
                .keys(doc! {
                    "owner": 1,
                    "launch.session.session_id": 1,
                    "created_at": -1,
                    "_id": -1,
                })
                .options(named("trigger_launches_by_owner_session"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "lease_expires_at": 1, "updated_at": 1 })
                .options(named("trigger_launches_runnable"))
                .build(),
            IndexModel::builder()
                .keys(doc! {
                    "current": 1,
                    "state": 1,
                    "updated_at": 1,
                    "created_at": -1,
                    "_id": -1,
                })
                .options(named("trigger_launches_worker_inventory"))
                .build(),
            IndexModel::builder()
                .keys(doc! {
                    "current": 1,
                    "state": 1,
                    "launch.reach.state": 1,
                    "created_at": -1,
                    "_id": -1,
                })
                .options(named("trigger_launches_capability_inventory"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "launch.events.correlation_id": 1 })
                .options(named("trigger_launches_by_correlation"))
                .build(),
            IndexModel::builder()
                .keys(doc! {
                    "current": 1,
                    "state": 1,
                    "launch.workspace.delete_after": 1,
                    "launch.cleanup.completed_at": 1,
                    "created_at": -1,
                })
                .options(named("trigger_launches_retention"))
                .build(),
        ] {
            self.launches
                .create_index(index)
                .await
                .map_err(|error| unavailable("create Launch index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "launch_id": 1, "sequence": 1 })
                .options(unique_named("trigger_launch_events_by_sequence"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "correlation_id": 1, "launch_id": 1 })
                .options(named("trigger_launch_events_by_correlation"))
                .build(),
        ] {
            self.launch_events
                .create_index(index)
                .await
                .map_err(|error| unavailable("create Launch event index", error))?;
        }
        self.connection_revocations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "owner": 1, "record.settled_at": 1, "service_id": 1 })
                    .options(named("trigger_connection_revocations_by_owner"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create connection revocation index", error))?;
        Ok(())
    }

    async fn draft_version(&self, id: &str) -> Result<Option<u64>, TriggerStoreError> {
        self.migrate_draft_schema(id).await?;
        let document = self
            .drafts
            .clone_with_type::<DraftVersionProjection>()
            .find_one(doc! { "_id": id })
            .projection(doc! { "document_schema_version": 1, "version": 1 })
            .await
            .map_err(|error| stored_read_failure("read Trigger draft revision", error))?;
        let Some(document) = document else {
            return Ok(None);
        };
        validate_document_schema_version(
            document.document_schema_version,
            TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
            "Trigger draft",
        )?;
        Ok(Some(unsigned(document.version, "Trigger draft revision")?))
    }

    async fn reconcile_publication(
        &self,
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
        unresolved: Error,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        self.migrate_revision_schema(&revision.id.0).await?;
        let persisted_revision = self
            .revisions
            .find_one(doc! { "_id": &revision.id.0 })
            .await
            .map_err(|error| {
                stored_read_failure("reconcile uncertain Trigger publication revision", error)
            })?
            .map(read_revision)
            .transpose()?;
        if let Some(persisted) = persisted_revision {
            if persisted == *revision {
                return Ok(PublishedRevision {
                    draft: draft.clone(),
                    revision: revision.clone(),
                });
            }
            return Err(TriggerStoreError::AlreadyExists);
        }

        let actual = self.draft_version(&draft.id.0).await?.unwrap_or(0);
        if actual != expected_draft_version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_draft_version,
                actual,
            });
        }
        Err(unavailable(
            "resolve uncertain Trigger publication transaction",
            unresolved,
        ))
    }

    /// Publish with bounded transaction/commit retries and durable ambiguity reconciliation.
    async fn publish_revision_transaction(
        &self,
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
        first_attempt_barrier: Option<&tokio::sync::Barrier>,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        let expected = signed(expected_draft_version, "Trigger draft expected revision")?;
        let draft_doc = draft_doc(draft)?;
        let revision_doc = revision_doc(revision)?;
        self.migrate_draft_schema(&draft.id.0).await?;

        let mut last_transient = None;
        for attempt in 0..TRIGGER_PUBLICATION_TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if error.contains_label(TRANSIENT_TRANSACTION_ERROR) => {
                    last_transient = Some(error);
                    continue;
                }
                Err(error) => {
                    return Err(unavailable("start Trigger publication transaction", error));
                }
            };
            if let Err(error) = session.start_transaction().await {
                if error.contains_label(TRANSIENT_TRANSACTION_ERROR) {
                    last_transient = Some(error);
                    continue;
                }
                return Err(unavailable("begin Trigger publication transaction", error));
            }

            let observed = self
                .drafts
                .clone_with_type::<DraftVersionProjection>()
                .find_one(doc! { "_id": &draft.id.0 })
                .projection(doc! { "document_schema_version": 1, "version": 1 })
                .session(&mut session)
                .await;
            let observed = match observed {
                Ok(Some(observed)) => observed,
                Ok(None) => {
                    let _ = session.abort_transaction().await;
                    return Err(TriggerStoreError::RevisionConflict {
                        expected: expected_draft_version,
                        actual: 0,
                    });
                }
                Err(error) if error.contains_label(TRANSIENT_TRANSACTION_ERROR) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error);
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(stored_read_failure(
                        "read Trigger draft in publication transaction",
                        error,
                    ));
                }
            };
            if let Err(error) = validate_document_schema_version(
                observed.document_schema_version,
                TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
                "Trigger draft",
            ) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            let actual = match unsigned(observed.version, "Trigger draft revision") {
                Ok(actual) => actual,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
            };
            if actual != expected_draft_version {
                let _ = session.abort_transaction().await;
                return Err(TriggerStoreError::RevisionConflict {
                    expected: expected_draft_version,
                    actual,
                });
            }
            if attempt == 0 {
                if let Some(barrier) = first_attempt_barrier {
                    barrier.wait().await;
                }
            }

            let replaced = self
                .drafts
                .replace_one(
                    doc! {
                        "_id": &draft.id.0,
                        "owner": &draft.owner.0,
                        "version": expected,
                        "document_schema_version": TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
                    },
                    &draft_doc,
                )
                .session(&mut session)
                .await;
            let replaced = match replaced {
                Ok(result) => result,
                Err(error) if error.contains_label(TRANSIENT_TRANSACTION_ERROR) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error);
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(unavailable("publish Trigger draft", error));
                }
            };
            if replaced.matched_count != 1 {
                let _ = session.abort_transaction().await;
                let actual = self.draft_version(&draft.id.0).await?.unwrap_or(0);
                return Err(TriggerStoreError::RevisionConflict {
                    expected: expected_draft_version,
                    actual,
                });
            }

            if let Err(error) = self
                .revisions
                .insert_one(&revision_doc)
                .session(&mut session)
                .await
            {
                let _ = session.abort_transaction().await;
                if error.contains_label(TRANSIENT_TRANSACTION_ERROR) {
                    last_transient = Some(error);
                    continue;
                }
                return Err(if is_duplicate_key(&error) {
                    TriggerStoreError::AlreadyExists
                } else {
                    unavailable("insert immutable Trigger revision", error)
                });
            }

            match commit_transaction(&mut session).await? {
                TransactionCommitOutcome::Committed => {
                    return Ok(PublishedRevision {
                        draft: read_draft(draft_doc)?,
                        revision: read_revision(revision_doc)?,
                    });
                }
                TransactionCommitOutcome::RetryTransaction(error) => {
                    last_transient = Some(error);
                }
                TransactionCommitOutcome::Unknown(error) => {
                    return self
                        .reconcile_publication(expected_draft_version, draft, revision, error)
                        .await;
                }
            }
        }

        let Some(last_transient) = last_transient else {
            return Err(TriggerStoreError::Unavailable(
                "Trigger publication retry budget ended without a driver result".into(),
            ));
        };
        self.reconcile_publication(expected_draft_version, draft, revision, last_transient)
            .await
    }

    async fn launch_after_failed_cas(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        expected_fence: Option<u64>,
    ) -> Result<TriggerStoreError, TriggerStoreError> {
        let Some(doc) = self
            .launches
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| unavailable("reconcile Launch write", error))?
        else {
            return Ok(TriggerStoreError::Malformed(
                "Launch disappeared during an atomic mutation".into(),
            ));
        };
        if let Some(expected) = expected_fence {
            let actual = unsigned(doc.fencing_counter, "Launch fencing counter")?;
            if actual != expected {
                return Ok(TriggerStoreError::StaleFence { expected, actual });
            }
        }
        let actual_revision = unsigned(doc.revision_number, "Launch revision")?;
        if actual_revision != expected_revision {
            return Ok(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
        Ok(TriggerStoreError::LeaseHeld)
    }

    async fn append_launch_events(
        &self,
        launch: &Launch,
        archived_through: u64,
        session: &mut mongodb::ClientSession,
    ) -> Result<(), TriggerStoreError> {
        let docs = launch_event_docs_after(launch, archived_through)?;
        if docs.is_empty() {
            return Ok(());
        }
        self.launch_events
            .insert_many(docs)
            .session(session)
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    TriggerStoreError::Malformed(
                        "Launch event sequence collided with canonical history".into(),
                    )
                } else {
                    unavailable("append Launch events", error)
                }
            })?;
        Ok(())
    }

    async fn fence_launch_for_revoked_connection(
        &self,
        id: &LaunchId,
        service_id: &str,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerStoreError> {
        for _ in 0..CONNECTION_REVOCATION_CAS_RETRIES {
            let Some(current) = self
                .launches
                .find_one(doc! { "_id": &id.0 })
                .await
                .map_err(|error| unavailable("read Launch for connection revocation", error))?
            else {
                return Ok(());
            };
            let mut launch = read_launch(current.clone())?;
            if launch
                .connection
                .as_ref()
                .is_none_or(|connection| connection.service_id != service_id)
            {
                return Ok(());
            }

            let mut probe = launch.clone();
            let semantic_change = probe
                .require_connection_reauthentication(service_id, now, correlation_id)
                .map_err(|error| {
                    TriggerStoreError::Malformed(format!(
                        "cannot fence Launch after connection revocation: {error}"
                    ))
                })?;
            if !semantic_change && launch.lease.is_none() {
                return Ok(());
            }

            launch
                .require_connection_reauthentication(service_id, now, correlation_id)
                .map_err(|error| {
                    TriggerStoreError::Malformed(format!(
                        "cannot apply connection revocation to Launch: {error}"
                    ))
                })?;
            let expected_revision = launch.revision_number;
            launch.revision_number = launch.revision_number.checked_add(1).ok_or_else(|| {
                TriggerStoreError::Malformed("Launch revision is exhausted".into())
            })?;
            let (update, _) = exact_launch_update(&launch)?;

            let mut session =
                self.client.start_session().await.map_err(|error| {
                    unavailable("start connection revocation transaction", error)
                })?;
            session
                .start_transaction()
                .await
                .map_err(|error| unavailable("begin connection revocation transaction", error))?;
            let previous = self
                .launches
                .find_one_and_update(
                    doc! {
                        "_id": &id.0,
                        "revision_number": signed(
                            expected_revision,
                            "revoked connection Launch revision",
                        )?,
                    },
                    update,
                )
                .session(&mut session)
                .await
                .map_err(|error| unavailable("fence revoked connection Launch", error))?;
            let Some(previous) = previous else {
                let _ = session.abort_transaction().await;
                continue;
            };
            let archived_through = previous
                .event_high_water
                .map(|value| unsigned(value, "Launch event high-water"))
                .transpose()?
                .unwrap_or(0);
            if let Err(error) = self
                .append_launch_events(&launch, archived_through, &mut session)
                .await
            {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            commit(&mut session, "commit connection revocation Launch fence").await?;
            return Ok(());
        }
        Err(TriggerStoreError::Unavailable(
            "Launch changed repeatedly while fencing a revoked connection".into(),
        ))
    }
}

#[async_trait::async_trait]
impl ConnectionRevocationStore for MongoTriggerStore {
    async fn insert_or_get_connection_revocation(
        &self,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
        let persisted = connection_revocation_doc(record)?;
        match self.connection_revocations.insert_one(persisted).await {
            Ok(_) => Ok(record.clone()),
            Err(error) if is_duplicate_key(&error) => self
                .get_connection_revocation(&record.owner, &record.service_id)
                .await?
                .ok_or_else(|| {
                    TriggerStoreError::Unavailable(
                        "connection revocation intent disappeared after conflict".into(),
                    )
                }),
            Err(error) => Err(unavailable("insert connection revocation intent", error)),
        }
    }

    async fn get_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<Option<ConnectionRevocationRecord>, TriggerStoreError> {
        self.connection_revocations
            .find_one(connection_revocation_filter(owner, service_id))
            .await
            .map_err(|error| unavailable("read connection revocation", error))?
            .map(read_connection_revocation)
            .transpose()
    }

    async fn restart_connection_revocation(
        &self,
        expected_revision: u64,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
        if record.revision
            != expected_revision.checked_add(1).ok_or_else(|| {
                TriggerStoreError::Malformed("connection revocation revision is exhausted".into())
            })?
            || record.provider_receipt.is_some()
            || record.settled_at.is_some()
            || record.dispatch_lease.is_some()
        {
            return Err(TriggerStoreError::Malformed(
                "restarted connection revocation epoch is invalid".into(),
            ));
        }
        let replacement = connection_revocation_doc(record)?;
        let expected = signed(expected_revision, "connection revocation expected revision")?;
        let mut filter = connection_revocation_filter(&record.owner, &record.service_id);
        filter.insert("revision", expected);
        filter.insert("record.settled_at", doc! { "$type": "string" });
        filter.insert(
            "record.provider_receipt.state_version",
            doc! { "$lt": record.active_snapshot.state_version },
        );
        let updated = self
            .connection_revocations
            .find_one_and_replace(filter, replacement)
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("restart connection revocation epoch", error))?;
        if let Some(updated) = updated {
            return read_connection_revocation(updated);
        }
        let actual = self
            .get_connection_revocation(&record.owner, &record.service_id)
            .await?
            .map_or(0, |record| record.revision);
        Err(TriggerStoreError::RevisionConflict {
            expected: expected_revision,
            actual,
        })
    }

    async fn claim_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
        lease: &ConnectionRevocationLease,
    ) -> Result<Option<ConnectionRevocationRecord>, TriggerStoreError> {
        validate_connection_revocation_lease(lease)?;
        let mut filter = connection_revocation_filter(owner, service_id);
        filter.insert(
            "$and",
            Bson::Array(vec![
                Bson::Document(doc! {
                    "$or": [
                        { "record.settled_at": Bson::Null },
                        { "record.settled_at": { "$exists": false } },
                    ],
                }),
                Bson::Document(doc! {
                    "$or": [
                        { "lease_expires_at": Bson::Null },
                        { "lease_expires_at": { "$exists": false } },
                        { "lease_expires_at": { "$lte": &lease.acquired_at } },
                    ],
                }),
            ]),
        );
        let lease_bson = mongodb::bson::serialize_to_bson(lease).map_err(|error| {
            TriggerStoreError::Malformed(format!("encode connection revocation lease: {error}"))
        })?;
        let updated = self
            .connection_revocations
            .find_one_and_update(
                filter,
                vec![doc! {
                    "$set": {
                        "revision": { "$add": ["$revision", 1_i64] },
                        "record.revision": { "$add": ["$record.revision", 1_i64] },
                        "lease_expires_at": &lease.expires_at,
                        "record.dispatch_lease": lease_bson,
                    },
                }],
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("claim connection revocation", error))?;
        updated.map(read_connection_revocation).transpose()
    }

    async fn record_connection_revocation_receipt(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
        replace_connection_revocation(
            &self.connection_revocations,
            expected_revision,
            lease_holder,
            record,
            false,
            "record connection revocation receipt",
        )
        .await
    }

    async fn fence_launches_for_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerStoreError> {
        let launches = self
            .launches
            .find(connection_revocation_launch_filter(owner, service_id))
            .await
            .map_err(|error| unavailable("list Launches for connection revocation", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read Launch revocation inventory", error))?;
        for launch in launches {
            self.fence_launch_for_revoked_connection(
                &LaunchId(launch.id),
                service_id,
                now,
                correlation_id,
            )
            .await?;
        }
        Ok(())
    }

    async fn settle_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
        replace_connection_revocation(
            &self.connection_revocations,
            expected_revision,
            lease_holder,
            record,
            true,
            "settle connection revocation",
        )
        .await
    }

    async fn release_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<(), TriggerStoreError> {
        let expected = signed(expected_revision, "connection revocation expected revision")?;
        let mut filter = connection_revocation_filter(owner, service_id);
        filter.insert("revision", expected);
        filter.insert("record.dispatch_lease.holder", lease_holder);
        let result = self
            .connection_revocations
            .update_one(
                filter,
                vec![doc! {
                    "$set": {
                        "revision": { "$add": ["$revision", 1_i64] },
                        "record.revision": { "$add": ["$record.revision", 1_i64] },
                        "lease_expires_at": Bson::Null,
                        "record.dispatch_lease": Bson::Null,
                    },
                }],
            )
            .await
            .map_err(|error| unavailable("release connection revocation", error))?;
        if result.matched_count == 1 {
            return Ok(());
        }
        let actual = self
            .get_connection_revocation(owner, service_id)
            .await?
            .map_or(0, |record| record.revision);
        Err(TriggerStoreError::RevisionConflict {
            expected: expected_revision,
            actual,
        })
    }

    async fn list_settled_connection_revocations(
        &self,
        owner: &SubjectId,
    ) -> Result<Vec<ConnectionRevocationRecord>, TriggerStoreError> {
        let docs = self
            .connection_revocations
            .find(doc! {
                "owner": &owner.0,
                "record.settled_at": { "$type": "string" },
            })
            .sort(doc! { "service_id": 1 })
            .await
            .map_err(|error| unavailable("list settled connection revocations", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read settled connection revocation", error))?;
        docs.into_iter().map(read_connection_revocation).collect()
    }
}

#[async_trait::async_trait]
impl TriggerStore for MongoTriggerStore {
    async fn insert_draft(&self, draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        self.drafts
            .insert_one(draft_doc(draft)?)
            .await
            .map_err(map_insert("insert Trigger draft"))?;
        Ok(())
    }

    async fn get_draft(&self, id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        self.migrate_draft_schema(&id.0).await?;
        self.drafts
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read Trigger draft", error))?
            .map(read_draft)
            .transpose()
    }

    async fn list_drafts_for(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError> {
        let filter = page_filter(
            doc! { "owner": &owner.0 },
            page.cursor.as_deref(),
            "updated_at",
        )?;
        let docs = self
            .drafts
            .find(filter)
            .sort(doc! { "updated_at": -1, "_id": -1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| unavailable("list Trigger drafts", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("read Trigger draft page", error))?;
        self.migrate_draft_page(&docs).await?;
        map_page(docs, page.limit, |doc| {
            let cursor = encode_cursor(&doc.updated_at, &doc.id);
            Ok((read_draft(doc)?, cursor))
        })
    }

    async fn replace_draft(
        &self,
        expected_version: u64,
        draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError> {
        let expected = signed(expected_version, "Trigger draft expected revision")?;
        let replacement = draft_doc(draft)?;
        self.migrate_draft_schema(&draft.id.0).await?;
        let result = self
            .drafts
            .find_one_and_replace(
                doc! {
                    "_id": &draft.id.0,
                    "owner": &draft.owner.0,
                    "version": expected,
                    "document_schema_version": TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
                },
                replacement,
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("replace Trigger draft", error))?;
        if let Some(doc) = result {
            return read_draft(doc);
        }
        let actual = self.draft_version(&draft.id.0).await?.unwrap_or(0);
        Err(TriggerStoreError::RevisionConflict {
            expected: expected_version,
            actual,
        })
    }

    async fn publish_revision(
        &self,
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        self.publish_revision_transaction(expected_draft_version, draft, revision, None)
            .await
    }

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.migrate_revision_schema(&id.0).await?;
        self.revisions
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read Trigger revision", error))?
            .map(read_revision)
            .transpose()
    }

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        let document = self
            .revisions
            .find_one(doc! { "public_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read public Trigger revision", error))?;
        if let Some(document) = &document {
            if document.document_schema_version == 0 {
                self.migrate_revision_schema(&document.id).await?;
            }
        }
        document.map(read_revision).transpose()
    }

    async fn list_revisions_for(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError> {
        let filter = page_filter(
            doc! { "owner": &owner.0 },
            page.cursor.as_deref(),
            "published_at",
        )?;
        let docs = self
            .revisions
            .find(filter)
            .sort(doc! { "published_at": -1, "_id": -1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| unavailable("list Trigger revisions", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("read Trigger revision page", error))?;
        self.migrate_revision_page(&docs).await?;
        map_page(docs, page.limit, |doc| {
            let cursor = encode_cursor(&doc.published_at, &doc.id);
            Ok((read_revision(doc)?, cursor))
        })
    }

    async fn retire_revision(
        &self,
        id: &TriggerRevisionId,
        owner: &SubjectId,
        retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.migrate_revision_schema(&id.0).await?;
        let updated = self
            .revisions
            .find_one_and_update(
                doc! {
                    "_id": &id.0,
                    "owner": &owner.0,
                    "document_schema_version": TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION,
                    "revision.availability": "published",
                },
                doc! {
                    "$set": {
                        "revision.availability": "retired",
                        "revision.retired_at": retired_at,
                    }
                },
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("retire Trigger revision", error))?;
        if let Some(doc) = updated {
            return read_revision(doc).map(Some);
        }
        self.revisions
            .find_one(doc! { "_id": &id.0, "owner": &owner.0 })
            .await
            .map_err(|error| stored_read_failure("reconcile retired Trigger revision", error))?
            .map(read_revision)
            .transpose()
    }
}

#[async_trait::async_trait]
impl LaunchStore for MongoTriggerStore {
    async fn insert_or_get_current(&self, launch: &Launch) -> Result<Launch, TriggerStoreError> {
        let persisted = launch_doc(launch, true)?;
        let returned = read_launch(persisted.clone())?;
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start current Launch transaction", error))?;
        session
            .start_transaction()
            .await
            .map_err(|error| unavailable("begin current Launch transaction", error))?;
        match self
            .launches
            .insert_one(persisted)
            .session(&mut session)
            .await
        {
            Ok(_) => {
                if let Err(error) = self.append_launch_events(launch, 0, &mut session).await {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                commit(&mut session, "commit current Launch").await?;
                Ok(returned)
            }
            Err(error) if is_duplicate_key(&error) => {
                let _ = session.abort_transaction().await;
                self.launches
                    .find_one(doc! {
                        "owner": &launch.owner.0,
                        "revision_id": &launch.revision.id.0,
                        "current": true,
                    })
                    .await
                    .map_err(|read| unavailable("reconcile current Launch", read))?
                    .map(read_launch)
                    .transpose()?
                    .ok_or(TriggerStoreError::AlreadyExists)
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                Err(unavailable("insert Launch", error))
            }
        }
    }

    async fn get_current_launch(
        &self,
        owner: &SubjectId,
        revision: &TriggerRevisionId,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        self.launches
            .find_one(doc! {
                "owner": &owner.0,
                "revision_id": &revision.0,
                "current": true,
            })
            .await
            .map_err(|error| unavailable("read current Launch", error))?
            .map(read_launch)
            .transpose()
    }

    async fn insert_new(&self, launch: &Launch) -> Result<(), TriggerStoreError> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start new Launch transaction", error))?;
        session
            .start_transaction()
            .await
            .map_err(|error| unavailable("begin new Launch transaction", error))?;
        if let Err(error) = self
            .launches
            .update_many(
                doc! {
                    "owner": &launch.owner.0,
                    "revision_id": &launch.revision.id.0,
                    "current": true,
                },
                doc! { "$set": { "current": false } },
            )
            .session(&mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(unavailable("supersede current Launch", error));
        }
        if let Err(error) = self
            .launches
            .insert_one(launch_doc(launch, true)?)
            .session(&mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(if is_duplicate_key(&error) {
                TriggerStoreError::AlreadyExists
            } else {
                unavailable("insert new Launch", error)
            });
        }
        if let Err(error) = self.append_launch_events(launch, 0, &mut session).await {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        commit(&mut session, "commit new Launch").await
    }

    async fn expire_current_and_insert(
        &self,
        expired: &Launch,
        replacement: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        if expired.owner != replacement.owner || expired.revision.id != replacement.revision.id {
            return Err(TriggerStoreError::Malformed(
                "expired Launch replacement changed owner or Trigger revision".into(),
            ));
        }
        let expected_revision = expired.revision_number.checked_sub(1).ok_or_else(|| {
            TriggerStoreError::Malformed(
                "expired Launch did not advance its optimistic revision".into(),
            )
        })?;
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start expired Launch transaction", error))?;
        session
            .start_transaction()
            .await
            .map_err(|error| unavailable("begin expired Launch transaction", error))?;
        let settled = self
            .launches
            .find_one_and_replace(
                doc! {
                    "_id": &expired.id.0,
                    "revision_number": signed(expected_revision, "expired Launch expected revision")?,
                    "current": true,
                },
                launch_doc(expired, false)?,
            )
            .session(&mut session)
            .await
            .map_err(|error| unavailable("settle expired Launch", error))?;
        let Some(settled) = settled else {
            let _ = session.abort_transaction().await;
            return self
                .get_current_launch(&replacement.owner, &replacement.revision.id)
                .await?
                .ok_or(TriggerStoreError::Superseded);
        };
        if let Err(error) = self
            .launches
            .insert_one(launch_doc(replacement, true)?)
            .session(&mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(if is_duplicate_key(&error) {
                TriggerStoreError::AlreadyExists
            } else {
                unavailable("insert replacement Launch", error)
            });
        }
        let archived_through = settled
            .event_high_water
            .map(|value| unsigned(value, "Launch event high-water"))
            .transpose()?
            .unwrap_or(0);
        if let Err(error) = self
            .append_launch_events(expired, archived_through, &mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        if let Err(error) = self
            .append_launch_events(replacement, 0, &mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        commit(&mut session, "commit expired Launch replacement").await?;
        read_launch(launch_doc(replacement, true)?)
    }

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
        self.launches
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| unavailable("read Launch", error))?
            .map(read_launch)
            .transpose()
    }

    async fn list_launch_events(
        &self,
        id: &LaunchId,
        after: u64,
        limit: usize,
    ) -> Result<Vec<LaunchEvent>, TriggerStoreError> {
        let after = signed(after, "Launch event cursor")?;
        self.launch_events
            .find(doc! {
                "launch_id": &id.0,
                "sequence": { "$gt": after },
            })
            .sort(doc! { "sequence": 1 })
            .limit(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
            .await
            .map_err(|error| unavailable("list Launch events", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read Launch events", error))?
            .into_iter()
            .map(read_launch_event)
            .collect()
    }

    async fn find_launch_by_correlation_id(
        &self,
        correlation_id: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let retained = self
            .launches
            .find_one(doc! {
                "$or": [
                    { "launch.events.correlation_id": correlation_id },
                    { "launch.steps.correlation_id": correlation_id },
                    { "launch.failure.correlation_id": correlation_id },
                    { "launch.effects.last_failure.correlation_id": correlation_id },
                ]
            })
            .await
            .map_err(|error| unavailable("read Launch by correlation", error))?
            .map(read_launch)
            .transpose()?;
        if retained.is_some() {
            return Ok(retained);
        }
        let Some(event) = self
            .launch_events
            .find_one(doc! { "correlation_id": correlation_id })
            .await
            .map_err(|error| unavailable("read Launch event by correlation", error))?
        else {
            return Ok(None);
        };
        let event = read_launch_event(event)?;
        self.get_launch(&event.launch_id).await
    }

    async fn list_launches(
        &self,
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let filter = launch_query_filter(query)?;
        let filter = page_filter(filter, query.page.cursor.as_deref(), "created_at")?;
        let docs = self
            .launches
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(i64::from(query.page.limit) + 1)
            .await
            .map_err(|error| unavailable("list Launches", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read Launch page", error))?;
        map_page(docs, query.page.limit, |doc| {
            let cursor = encode_cursor(&doc.created_at, &doc.id);
            Ok((read_launch(doc)?, cursor))
        })
    }

    async fn list_worker_candidates(
        &self,
        updated_before: Option<String>,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let states = mongodb::bson::serialize_to_bson(&TRIGGER_WORKER_STATES).map_err(|error| {
            TriggerStoreError::Malformed(format!("encode worker Launch states: {error}"))
        })?;
        let filter = worker_candidate_filter(states, updated_before.as_deref());
        let filter = page_filter(filter, page.cursor.as_deref(), "created_at")?;
        let docs = self
            .launches
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| unavailable("list worker Launch candidates", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read worker Launch candidate page", error))?;
        map_page(docs, page.limit, |doc| {
            let cursor = encode_cursor(&doc.created_at, &doc.id);
            Ok((read_launch(doc)?, cursor))
        })
    }

    async fn list_capability_candidates(
        &self,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let states =
            mongodb::bson::serialize_to_bson(&TRIGGER_CAPABILITY_STATES).map_err(|error| {
                TriggerStoreError::Malformed(format!("encode capability Launch states: {error}"))
            })?;
        let revoked = mongodb::bson::serialize_to_bson(&CapabilityRevocationState::Revoked)
            .map_err(|error| {
                TriggerStoreError::Malformed(format!("encode revoked reach state: {error}"))
            })?;
        let terminal_states = mongodb::bson::serialize_to_bson(&[
            LaunchState::Completed,
            LaunchState::FailedTerminal,
            LaunchState::Cancelled,
        ])
        .map_err(|error| {
            TriggerStoreError::Malformed(format!("encode terminal Launch states: {error}"))
        })?;
        let cancelled =
            mongodb::bson::serialize_to_bson(&LaunchState::Cancelled).map_err(|error| {
                TriggerStoreError::Malformed(format!("encode cancelled Launch state: {error}"))
            })?;
        let settled_prompt_states = mongodb::bson::serialize_to_bson(&[
            layer2_core::PromptDeliveryState::Completed,
            layer2_core::PromptDeliveryState::Failed,
        ])
        .map_err(|error| {
            TriggerStoreError::Malformed(format!("encode settled prompt states: {error}"))
        })?;
        let filter = page_filter(
            capability_candidate_filter(
                states,
                revoked,
                terminal_states,
                cancelled,
                settled_prompt_states,
            ),
            page.cursor.as_deref(),
            "created_at",
        )?;
        let docs = self
            .launches
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| unavailable("list capability Launch candidates", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read capability Launch candidate page", error))?;
        map_page(docs, page.limit, |doc| {
            let cursor = encode_cursor(&doc.created_at, &doc.id);
            Ok((read_launch(doc)?, cursor))
        })
    }

    async fn list_cleanup_candidates(
        &self,
        delete_before: &str,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let states = mongodb::bson::serialize_to_bson(&[
            LaunchState::Completed,
            LaunchState::FailedTerminal,
            LaunchState::Cancelled,
        ])
        .map_err(|error| {
            TriggerStoreError::Malformed(format!("encode terminal states: {error}"))
        })?;
        let filter = page_filter(
            cleanup_candidate_filter(states, delete_before),
            page.cursor.as_deref(),
            "created_at",
        )?;
        let docs = self
            .launches
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| unavailable("list cleanup candidates", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read cleanup candidate page", error))?;
        map_page(docs, page.limit, |doc| {
            let cursor = encode_cursor(&doc.created_at, &doc.id);
            Ok((read_launch(doc)?, cursor))
        })
    }

    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        let expected_revision_i64 = signed(expected_revision, "Launch expected revision")?;
        let mut filter = doc! {
            "_id": &launch.id.0,
            "revision_number": expected_revision_i64,
        };
        match expected_fence {
            Some(fence) => {
                filter.insert("fencing_counter", signed(fence, "Launch expected fence")?);
                filter.insert(
                    "launch.lease.fencing_token",
                    signed(fence, "Launch lease fence")?,
                );
            }
            None => {
                filter.insert(
                    "$or",
                    Bson::Array(vec![
                        Bson::Document(doc! { "launch.lease": Bson::Null }),
                        Bson::Document(doc! { "launch.lease": { "$exists": false } }),
                    ]),
                );
            }
        }
        let (update, returned) = exact_launch_update(launch)?;
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start Launch replacement transaction", error))?;
        session
            .start_transaction()
            .await
            .map_err(|error| unavailable("begin Launch replacement transaction", error))?;
        let previous = self
            .launches
            .find_one_and_update(filter, update)
            .session(&mut session)
            .await
            .map_err(|error| unavailable("replace Launch", error))?;
        let Some(previous) = previous else {
            let _ = session.abort_transaction().await;
            return Err(self
                .launch_after_failed_cas(&launch.id, expected_revision, expected_fence)
                .await?);
        };
        let archived_through = previous
            .event_high_water
            .map(|value| unsigned(value, "Launch event high-water"))
            .transpose()?
            .unwrap_or(0);
        if let Err(error) = self
            .append_launch_events(launch, archived_through, &mut session)
            .await
        {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
        commit(&mut session, "commit Launch replacement").await?;
        Ok(returned)
    }

    async fn acquire_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let expected = signed(expected_revision, "Launch expected revision")?;
        let next = expected_revision
            .checked_add(1)
            .ok_or_else(|| TriggerStoreError::Malformed("Launch revision overflowed".into()))?;
        let next = signed(next, "Launch next revision")?;
        let pipeline = vec![doc! {
            "$set": {
                "revision_number": next,
                "launch.revision_number": next,
                "fencing_counter": { "$add": ["$fencing_counter", 1_i64] },
                "launch.fencing_counter": { "$add": ["$launch.fencing_counter", 1_i64] },
                "lease_expires_at": expires_at,
                "launch.lease": {
                    "holder": holder,
                    "fencing_token": { "$add": ["$launch.fencing_counter", 1_i64] },
                    "acquired_at": acquired_at,
                    "expires_at": expires_at,
                },
            }
        }];
        let updated = self
            .launches
            .find_one_and_update(
                doc! {
                    "_id": &id.0,
                    "revision_number": expected,
                    "$or": [
                        { "lease_expires_at": Bson::Null },
                        { "lease_expires_at": { "$exists": false } },
                        { "lease_expires_at": { "$lte": acquired_at } },
                    ],
                },
                pipeline,
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("acquire Launch lease", error))?;
        updated.map(read_launch).transpose()
    }

    async fn release_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError> {
        let expected = signed(expected_revision, "Launch expected revision")?;
        let next = signed(
            expected_revision
                .checked_add(1)
                .ok_or_else(|| TriggerStoreError::Malformed("Launch revision overflowed".into()))?,
            "Launch next revision",
        )?;
        let fence = signed(lease.fencing_token, "Launch lease fence")?;
        let updated = self
            .launches
            .find_one_and_update(
                doc! {
                    "_id": &id.0,
                    "revision_number": expected,
                    "fencing_counter": fence,
                    "launch.lease.holder": &lease.holder,
                    "launch.lease.fencing_token": fence,
                },
                doc! {
                    "$set": {
                        "revision_number": next,
                        "launch.revision_number": next,
                        "lease_expires_at": Bson::Null,
                        "launch.lease": Bson::Null,
                    }
                },
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("release Launch lease", error))?;
        if let Some(doc) = updated {
            return read_launch(doc);
        }
        Err(self
            .launch_after_failed_cas(id, expected_revision, Some(lease.fencing_token))
            .await?)
    }
}

fn launch_query_filter(query: &LaunchQuery) -> Result<Document, TriggerStoreError> {
    let mut filter = Document::new();
    if let Some(owner) = query.owner.as_ref() {
        filter.insert("owner", owner.0.clone());
    }
    if let Some(public_id) = query.public_id.as_ref() {
        filter.insert("launch.revision.public_id", public_id.0.clone());
    }
    if let Some(session_id) = query.session_id.as_ref() {
        filter.insert("launch.session.session_id", session_id.0.clone());
    }
    if !query.states.is_empty() {
        filter.insert(
            "state",
            doc! { "$in": mongodb::bson::serialize_to_bson(&query.states).map_err(|error| {
                TriggerStoreError::Malformed(format!("encode Launch states: {error}"))
            })? },
        );
    }
    if let Some(bound) = query.updated_before.as_ref() {
        filter.insert("updated_at", doc! { "$lt": bound });
    }
    Ok(filter)
}

fn connection_revocation_filter(owner: &SubjectId, service_id: &str) -> Document {
    doc! {
        "_id": {
            "owner": &owner.0,
            "service_id": service_id,
        },
    }
}

fn connection_revocation_launch_filter(owner: &SubjectId, service_id: &str) -> Document {
    doc! {
        "owner": &owner.0,
        "launch.connection.service_id": service_id,
    }
}

fn worker_candidate_filter(states: Bson, updated_before: Option<&str>) -> Document {
    let mut clauses = vec![doc! { "state": { "$in": states } }];
    if let Some(bound) = updated_before {
        clauses.push(doc! { "updated_at": { "$lt": bound } });
        clauses.push(doc! {
            "$or": [
                { "state": { "$ne": "pr_open" } },
                {
                    "state": "pr_open",
                    "launch.pull_request.state": "open",
                    "$or": [
                        { "launch.pull_request.status_sync.next_attempt_at": { "$lte": bound } },
                        { "launch.pull_request.status_sync.next_attempt_at": { "$exists": false } },
                        { "launch.pull_request.status_sync.next_attempt_at": Bson::Null },
                    ],
                },
            ],
        });
    }
    doc! { "$and": clauses }
}

fn capability_candidate_filter(
    states: Bson,
    revoked: Bson,
    terminal_states: Bson,
    cancelled: Bson,
    settled_prompt_states: Bson,
) -> Document {
    doc! {
        "state": { "$in": states },
        "$or": [
            { "launch.reach.state": { "$exists": true, "$ne": revoked } },
            {
                "state": { "$in": terminal_states },
                "launch.session_credential_retirement": { "$exists": false },
            },
            {
                "state": cancelled,
                "launch.session": { "$exists": true, "$ne": Bson::Null },
                "launch.delivery.state": { "$nin": settled_prompt_states },
            },
        ],
    }
}

fn cleanup_candidate_filter(states: Bson, delete_before: &str) -> Document {
    doc! {
        "state": { "$in": states },
        "$and": [
            {
                "$or": [
                    { "launch.workspace.delete_after": { "$lte": delete_before } },
                    {
                        "launch.workspace": { "$in": [Bson::Null] },
                        "launch.terminal_at": { "$lte": delete_before },
                        "launch.effects": {
                            "$elemMatch": { "kind": "workspace_import" },
                        },
                    },
                    {
                        "launch.workspace": { "$exists": false },
                        "launch.terminal_at": { "$lte": delete_before },
                        "launch.effects": {
                            "$elemMatch": { "kind": "workspace_import" },
                        },
                    },
                ],
            },
            {
                "$or": [
                    { "launch.cleanup": { "$exists": false } },
                    { "launch.cleanup": Bson::Null },
                    { "launch.cleanup.completed_at": { "$exists": false } },
                    { "launch.cleanup.completed_at": Bson::Null },
                ],
            },
        ],
    }
}

fn validate_connection_revocation_lease(
    lease: &ConnectionRevocationLease,
) -> Result<(), TriggerStoreError> {
    if lease.holder.trim().is_empty()
        || layer2_core::seconds_from_rfc3339(&lease.acquired_at).is_none()
        || layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none()
        || lease.expires_at <= lease.acquired_at
    {
        return Err(TriggerStoreError::Malformed(
            "connection revocation lease is invalid".into(),
        ));
    }
    Ok(())
}

fn connection_revocation_doc(
    record: &ConnectionRevocationRecord,
) -> Result<ConnectionRevocationDoc, TriggerStoreError> {
    if record.owner.0.trim().is_empty()
        || record.service_id.trim().is_empty()
        || record.revision == 0
        || record.active_snapshot.service_id != record.service_id
        || record.active_snapshot.state_version <= 0
        || layer2_core::seconds_from_rfc3339(&record.requested_at).is_none()
    {
        return Err(TriggerStoreError::Malformed(
            "connection revocation owner proof is invalid".into(),
        ));
    }
    if let Some(lease) = record.dispatch_lease.as_ref() {
        validate_connection_revocation_lease(lease)?;
    }
    if let Some(receipt) = record.provider_receipt.as_ref() {
        if receipt.service_id != record.service_id
            || receipt.state != layer2_core::ConnectionState::Inactive
            || receipt.state_version <= record.active_snapshot.state_version
        {
            return Err(TriggerStoreError::Malformed(
                "connection revocation receipt is not positive provider evidence".into(),
            ));
        }
    }
    if let Some(settled_at) = record.settled_at.as_ref() {
        if layer2_core::seconds_from_rfc3339(settled_at).is_none()
            || record.provider_receipt.is_none()
            || record.dispatch_lease.is_some()
        {
            return Err(TriggerStoreError::Malformed(
                "settled connection revocation is incomplete".into(),
            ));
        }
    }
    Ok(ConnectionRevocationDoc {
        id: ConnectionRevocationIdDoc {
            owner: record.owner.0.clone(),
            service_id: record.service_id.clone(),
        },
        owner: record.owner.0.clone(),
        service_id: record.service_id.clone(),
        revision: signed(record.revision, "connection revocation revision")?,
        lease_expires_at: record
            .dispatch_lease
            .as_ref()
            .map(|lease| lease.expires_at.clone()),
        record: record.clone(),
    })
}

fn read_connection_revocation(
    doc: ConnectionRevocationDoc,
) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
    let revision = unsigned(doc.revision, "connection revocation revision")?;
    if doc.id.owner != doc.owner
        || doc.id.service_id != doc.service_id
        || doc.owner != doc.record.owner.0
        || doc.service_id != doc.record.service_id
        || revision != doc.record.revision
        || doc.lease_expires_at
            != doc
                .record
                .dispatch_lease
                .as_ref()
                .map(|lease| lease.expires_at.clone())
    {
        return Err(TriggerStoreError::Malformed(
            "connection revocation query fields disagree with its record".into(),
        ));
    }
    connection_revocation_doc(&doc.record)?;
    Ok(doc.record)
}

async fn replace_connection_revocation(
    collection: &Collection<ConnectionRevocationDoc>,
    expected_revision: u64,
    lease_holder: &str,
    record: &ConnectionRevocationRecord,
    settling: bool,
    context: &'static str,
) -> Result<ConnectionRevocationRecord, TriggerStoreError> {
    if lease_holder.trim().is_empty()
        || record.revision
            != expected_revision.checked_add(1).ok_or_else(|| {
                TriggerStoreError::Malformed("connection revocation revision is exhausted".into())
            })?
        || record.provider_receipt.is_none()
        || (settling && (record.settled_at.is_none() || record.dispatch_lease.is_some()))
        || (!settling
            && (record.settled_at.is_some()
                || record
                    .dispatch_lease
                    .as_ref()
                    .is_none_or(|lease| lease.holder != lease_holder)))
    {
        return Err(TriggerStoreError::Malformed(format!(
            "{context} has an invalid state transition"
        )));
    }
    let replacement = connection_revocation_doc(record)?;
    let expected = signed(expected_revision, "connection revocation expected revision")?;
    let mut filter = connection_revocation_filter(&record.owner, &record.service_id);
    filter.insert("revision", expected);
    filter.insert("record.dispatch_lease.holder", lease_holder);
    filter.insert(
        "$or",
        Bson::Array(vec![
            Bson::Document(doc! { "record.settled_at": Bson::Null }),
            Bson::Document(doc! { "record.settled_at": { "$exists": false } }),
        ]),
    );
    let updated = collection
        .find_one_and_replace(filter, replacement)
        .return_document(ReturnDocument::After)
        .await
        .map_err(|error| unavailable(context, error))?;
    if let Some(updated) = updated {
        return read_connection_revocation(updated);
    }
    let actual = collection
        .find_one(connection_revocation_filter(
            &record.owner,
            &record.service_id,
        ))
        .await
        .map_err(|error| unavailable("reconcile connection revocation CAS", error))?
        .map(read_connection_revocation)
        .transpose()?
        .map_or(0, |record| record.revision);
    Err(TriggerStoreError::RevisionConflict {
        expected: expected_revision,
        actual,
    })
}

fn draft_doc(draft: &TriggerDraft) -> Result<DraftDoc, TriggerStoreError> {
    Ok(DraftDoc {
        id: draft.id.0.clone(),
        document_schema_version: TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
        owner: draft.owner.0.clone(),
        version: signed(draft.version, "Trigger draft revision")?,
        updated_at: draft.updated_at.clone(),
        draft: draft.clone(),
    })
}

fn read_draft(doc: DraftDoc) -> Result<TriggerDraft, TriggerStoreError> {
    validate_document_schema_version(
        doc.document_schema_version,
        TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION,
        "Trigger draft",
    )?;
    if doc.id != doc.draft.id.0
        || doc.owner != doc.draft.owner.0
        || unsigned(doc.version, "Trigger draft revision")? != doc.draft.version
        || doc.updated_at != doc.draft.updated_at
    {
        return Err(TriggerStoreError::Malformed(
            "Trigger draft query fields disagree with its aggregate".into(),
        ));
    }
    doc.draft.content.validate().map_err(|error| {
        TriggerStoreError::Malformed(format!("stored Trigger draft is invalid: {error}"))
    })?;
    Ok(doc.draft)
}

fn revision_doc(revision: &TriggerRevision) -> Result<RevisionDoc, TriggerStoreError> {
    revision.validate().map_err(|error| {
        TriggerStoreError::Malformed(format!("Trigger revision is invalid: {error}"))
    })?;
    Ok(RevisionDoc {
        id: revision.id.0.clone(),
        document_schema_version: TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION,
        public_id: revision.public_id.0.clone(),
        owner: revision.content.publisher_subject_id.0.clone(),
        published_at: revision.published_at.clone(),
        revision: revision.clone(),
    })
}

fn read_revision(doc: RevisionDoc) -> Result<TriggerRevision, TriggerStoreError> {
    validate_document_schema_version(
        doc.document_schema_version,
        TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION,
        "Trigger revision",
    )?;
    doc.revision.validate().map_err(|error| {
        TriggerStoreError::Malformed(format!("stored Trigger revision is invalid: {error}"))
    })?;
    if doc.id != doc.revision.id.0
        || doc.public_id != doc.revision.public_id.0
        || doc.owner != doc.revision.content.publisher_subject_id.0
        || doc.published_at != doc.revision.published_at
    {
        return Err(TriggerStoreError::Malformed(
            "Trigger revision query fields or digest disagree with its aggregate".into(),
        ));
    }
    Ok(doc.revision)
}

fn legacy_document_schema_filter() -> Document {
    doc! {
        "$or": [
            { "document_schema_version": { "$exists": false } },
            {
                "$expr": {
                    "$and": [
                        {
                            "$in": [
                                { "$type": "$document_schema_version" },
                                ["int", "long"],
                            ],
                        },
                        { "$eq": ["$document_schema_version", 0_i64] },
                    ],
                },
            },
        ],
    }
}

fn exact_legacy_document_schema_filter(id: &str) -> Document {
    doc! {
        "$and": [
            { "_id": id },
            legacy_document_schema_filter(),
        ],
    }
}

fn current_document_schema_update(version: i64) -> Document {
    doc! {
        "$set": {
            "document_schema_version": version,
        },
    }
}

fn validate_document_schema_version(
    version: i64,
    current: i64,
    record: &str,
) -> Result<(), TriggerStoreError> {
    if !(0..=current).contains(&version) {
        return Err(TriggerStoreError::Malformed(format!(
            "stored {record} uses unsupported document schema version {version}"
        )));
    }
    Ok(())
}

fn launch_doc(launch: &Launch, current: bool) -> Result<LaunchDoc, TriggerStoreError> {
    let launch = retained_launch(launch)?;
    Ok(LaunchDoc {
        id: launch.id.0.clone(),
        owner: launch.owner.0.clone(),
        revision_id: launch.revision.id.0.clone(),
        current,
        state: launch.state,
        created_at: launch.created_at.clone(),
        updated_at: launch.updated_at.clone(),
        revision_number: signed(launch.revision_number, "Launch revision")?,
        fencing_counter: signed(launch.fencing_counter, "Launch fencing counter")?,
        lease_expires_at: launch.lease.as_ref().map(|lease| lease.expires_at.clone()),
        event_high_water: Some(signed(
            launch.event_high_water(),
            "Launch event high-water",
        )?),
        launch,
    })
}

/// Builds an exact-ID aggregate update without touching the independent
/// README-click selector. A superseded Launch must remain mutable for worker,
/// credential, revocation, recovery, and cleanup settlement.
fn exact_launch_update(launch: &Launch) -> Result<(Document, Launch), TriggerStoreError> {
    let persisted = launch_doc(launch, false)?;
    let returned = read_launch(persisted.clone())?;
    let fields = mongodb::bson::serialize_to_document(&persisted).map_err(|error| {
        TriggerStoreError::Malformed(format!("encode Launch replacement: {error}"))
    })?;
    Ok((preserve_current_selector(fields), returned))
}

fn preserve_current_selector(mut fields: Document) -> Document {
    fields.remove("_id");
    fields.remove("current");
    doc! { "$set": fields }
}

fn read_launch(mut doc: LaunchDoc) -> Result<Launch, TriggerStoreError> {
    let embedded_high_water = doc.launch.event_high_water();
    let event_high_water = doc
        .event_high_water
        .map(|value| unsigned(value, "Launch event high-water"))
        .transpose()?
        .unwrap_or(embedded_high_water);
    if doc.event_high_water.is_some() && embedded_high_water != event_high_water {
        return Err(TriggerStoreError::Malformed(
            "Launch event high-water disagrees with its aggregate tail".into(),
        ));
    }
    doc.launch.event_sequence = event_high_water;
    validate_event_tail(&doc.launch)?;
    if doc.id != doc.launch.id.0
        || doc.owner != doc.launch.owner.0
        || doc.revision_id != doc.launch.revision.id.0
        || doc.state != doc.launch.state
        || doc.created_at != doc.launch.created_at
        || doc.updated_at != doc.launch.updated_at
        || unsigned(doc.revision_number, "Launch revision")? != doc.launch.revision_number
        || unsigned(doc.fencing_counter, "Launch fencing counter")? != doc.launch.fencing_counter
        || doc.lease_expires_at
            != doc
                .launch
                .lease
                .as_ref()
                .map(|lease| lease.expires_at.clone())
    {
        return Err(TriggerStoreError::Malformed(
            "Launch query fields disagree with its aggregate".into(),
        ));
    }
    Ok(doc.launch)
}

fn retained_launch(launch: &Launch) -> Result<Launch, TriggerStoreError> {
    validate_event_tail(launch)?;
    let mut retained = launch.clone();
    retained.event_sequence = retained.event_high_water();
    trim_event_tail(&mut retained.events);
    Ok(retained)
}

fn trim_event_tail(events: &mut Vec<LaunchEvent>) {
    if events.len() > LAUNCH_EVENT_TAIL_LIMIT {
        events.drain(..events.len() - LAUNCH_EVENT_TAIL_LIMIT);
    }
}

fn validate_event_tail(launch: &Launch) -> Result<(), TriggerStoreError> {
    let high_water = launch.event_high_water();
    let Some(last) = launch.events.last() else {
        return Err(TriggerStoreError::Malformed(
            "Launch has no retained event tail".into(),
        ));
    };
    if last.sequence != high_water
        || launch
            .events
            .iter()
            .any(|event| event.launch_id != launch.id || event.sequence == 0)
        || launch.events.windows(2).any(|pair| {
            pair[0]
                .sequence
                .checked_add(1)
                .is_none_or(|next| next != pair[1].sequence)
        })
    {
        return Err(TriggerStoreError::Malformed(
            "Launch retained event tail is not contiguous at its high-water".into(),
        ));
    }
    Ok(())
}

fn launch_event_docs_after(
    launch: &Launch,
    archived_through: u64,
) -> Result<Vec<LaunchEventDoc>, TriggerStoreError> {
    validate_event_tail(launch)?;
    let high_water = launch.event_high_water();
    if archived_through > high_water {
        return Err(TriggerStoreError::Malformed(
            "canonical Launch event history is ahead of the aggregate".into(),
        ));
    }
    let mut expected = archived_through
        .checked_add(1)
        .ok_or_else(|| TriggerStoreError::Malformed("Launch event sequence is exhausted".into()))?;
    let mut docs = Vec::new();
    for event in launch
        .events
        .iter()
        .filter(|event| event.sequence > archived_through)
    {
        if event.sequence != expected {
            return Err(TriggerStoreError::Malformed(
                "Launch event append does not continue canonical history".into(),
            ));
        }
        docs.push(launch_event_doc(event)?);
        expected = expected.checked_add(1).ok_or_else(|| {
            TriggerStoreError::Malformed("Launch event sequence is exhausted".into())
        })?;
    }
    if expected.checked_sub(1) != Some(high_water) {
        return Err(TriggerStoreError::Malformed(
            "Launch event append is missing canonical history".into(),
        ));
    }
    Ok(docs)
}

fn launch_event_doc(event: &LaunchEvent) -> Result<LaunchEventDoc, TriggerStoreError> {
    Ok(LaunchEventDoc {
        id: launch_event_id(&event.launch_id, event.sequence),
        launch_id: event.launch_id.0.clone(),
        sequence: signed(event.sequence, "Launch event sequence")?,
        correlation_id: event.correlation_id.clone(),
        event: event.clone(),
    })
}

fn read_launch_event(doc: LaunchEventDoc) -> Result<LaunchEvent, TriggerStoreError> {
    if doc.id != launch_event_id(&doc.event.launch_id, doc.event.sequence)
        || doc.launch_id != doc.event.launch_id.0
        || unsigned(doc.sequence, "Launch event sequence")? != doc.event.sequence
        || doc.correlation_id != doc.event.correlation_id
    {
        return Err(TriggerStoreError::Malformed(
            "Launch event query fields disagree with its envelope".into(),
        ));
    }
    Ok(doc.event)
}

fn launch_event_id(launch_id: &LaunchId, sequence: u64) -> String {
    format!("{}:{sequence:020}", launch_id.0)
}

fn signed(value: u64, field: &str) -> Result<i64, TriggerStoreError> {
    i64::try_from(value)
        .map_err(|_| TriggerStoreError::Malformed(format!("{field} exceeds MongoDB's range")))
}

fn unsigned(value: i64, field: &str) -> Result<u64, TriggerStoreError> {
    u64::try_from(value)
        .map_err(|_| TriggerStoreError::Malformed(format!("{field} must not be negative")))
}

fn named(name: &str) -> IndexOptions {
    IndexOptions::builder().name(name.to_string()).build()
}

fn unique_named(name: &str) -> IndexOptions {
    IndexOptions::builder()
        .name(name.to_string())
        .unique(true)
        .build()
}

fn unavailable(context: &str, error: Error) -> TriggerStoreError {
    TriggerStoreError::Unavailable(format!("{context}: {error}"))
}

fn stored_read_failure(context: &str, error: Error) -> TriggerStoreError {
    match error.kind.as_ref() {
        ErrorKind::BsonDeserialization(_) | ErrorKind::Bson(_) => {
            TriggerStoreError::Malformed(format!("{context}: stored document cannot be decoded"))
        }
        _ => unavailable(context, error),
    }
}

fn map_insert(context: &'static str) -> impl FnOnce(Error) -> TriggerStoreError {
    move |error| {
        if is_duplicate_key(&error) {
            TriggerStoreError::AlreadyExists
        } else {
            unavailable(context, error)
        }
    }
}

fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

async fn commit_transaction(
    session: &mut mongodb::ClientSession,
) -> Result<TransactionCommitOutcome, TriggerStoreError> {
    for attempt in 0..TRIGGER_PUBLICATION_COMMIT_ATTEMPTS {
        match session.commit_transaction().await {
            Ok(()) => return Ok(TransactionCommitOutcome::Committed),
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => {
                if attempt + 1 == TRIGGER_PUBLICATION_COMMIT_ATTEMPTS {
                    return Ok(TransactionCommitOutcome::Unknown(error));
                }
                tokio::task::yield_now().await;
            }
            Err(error) if error.contains_label(TRANSIENT_TRANSACTION_ERROR) => {
                return Ok(TransactionCommitOutcome::RetryTransaction(error));
            }
            Err(error) => return Err(unavailable("commit Trigger publication", error)),
        }
    }
    unreachable!("the bounded Trigger publication commit loop always returns")
}

async fn commit(
    session: &mut mongodb::ClientSession,
    context: &str,
) -> Result<(), TriggerStoreError> {
    match commit_launch_transaction(session).await {
        Ok(()) => Ok(()),
        Err(LaunchCommitFailure::Unknown(error)) => Err(TriggerStoreError::Unavailable(format!(
            "{context}: transaction commit result remains unknown after \
             {LAUNCH_TRANSACTION_COMMIT_ATTEMPTS} attempts: {error}"
        ))),
        Err(LaunchCommitFailure::Deadline(Some(error))) => {
            Err(TriggerStoreError::Unavailable(format!(
                "{context}: transaction commit result remains unknown at the \
                 {}ms deadline: {error}",
                LAUNCH_TRANSACTION_COMMIT_DEADLINE.as_millis()
            )))
        }
        Err(LaunchCommitFailure::Deadline(None)) => Err(TriggerStoreError::Unavailable(format!(
            "{context}: transaction commit result remains unknown at the {}ms deadline",
            LAUNCH_TRANSACTION_COMMIT_DEADLINE.as_millis()
        ))),
        Err(LaunchCommitFailure::Failed(error)) => Err(unavailable(context, error)),
    }
}

async fn commit_launch_transaction(
    committer: &mut (impl LaunchTransactionCommitter + Send),
) -> Result<(), LaunchCommitFailure> {
    commit_launch_transaction_with_policy(
        committer,
        LAUNCH_TRANSACTION_COMMIT_ATTEMPTS,
        LAUNCH_TRANSACTION_COMMIT_DEADLINE,
        LAUNCH_TRANSACTION_COMMIT_INITIAL_BACKOFF,
        LAUNCH_TRANSACTION_COMMIT_MAX_BACKOFF,
    )
    .await
}

async fn commit_launch_transaction_with_policy(
    committer: &mut (impl LaunchTransactionCommitter + Send),
    attempts: usize,
    deadline_after: Duration,
    initial_backoff: Duration,
    max_backoff: Duration,
) -> Result<(), LaunchCommitFailure> {
    debug_assert!(attempts > 0);
    debug_assert!(initial_backoff <= max_backoff);

    let deadline = tokio::time::Instant::now() + deadline_after;
    let mut backoff = initial_backoff;
    let mut last_unknown = None;
    for attempt in 0..attempts {
        match tokio::time::timeout_at(deadline, committer.commit_once()).await {
            Ok(LaunchCommitAttempt::Committed) => return Ok(()),
            Ok(LaunchCommitAttempt::Unknown(error)) => {
                last_unknown = Some(error);
                if attempt + 1 == attempts {
                    return Err(LaunchCommitFailure::Unknown(
                        last_unknown.expect("the unknown result was recorded above"),
                    ));
                }
            }
            Ok(LaunchCommitAttempt::Failed(error)) => {
                return Err(LaunchCommitFailure::Failed(error));
            }
            Err(_) => return Err(LaunchCommitFailure::Deadline(last_unknown)),
        }

        let now = tokio::time::Instant::now();
        let Some(wake_at) = now.checked_add(backoff) else {
            return Err(LaunchCommitFailure::Deadline(last_unknown));
        };
        if wake_at >= deadline {
            return Err(LaunchCommitFailure::Deadline(last_unknown));
        }
        tokio::time::sleep_until(wake_at).await;
        backoff = std::cmp::min(backoff * 2, max_backoff);
    }
    unreachable!("the bounded Launch commit loop always returns")
}

fn encode_cursor(stamp: &str, id: &str) -> String {
    format!("{stamp}|{id}")
}

fn decode_cursor(cursor: &str) -> Result<(&str, &str), TriggerStoreError> {
    let (stamp, id) = cursor
        .split_once('|')
        .ok_or_else(|| TriggerStoreError::Malformed("Trigger page cursor is not usable".into()))?;
    if stamp.is_empty() || id.is_empty() || id.contains('|') {
        return Err(TriggerStoreError::Malformed(
            "Trigger page cursor is not usable".into(),
        ));
    }
    Ok((stamp, id))
}

fn page_filter(
    base: Document,
    cursor: Option<&str>,
    time_field: &'static str,
) -> Result<Document, TriggerStoreError> {
    let Some(cursor) = cursor else {
        return Ok(base);
    };
    let (stamp, id) = decode_cursor(cursor)?;
    let mut older = Document::new();
    older.insert(time_field, doc! { "$lt": stamp });
    let mut same_time = Document::new();
    same_time.insert(time_field, stamp);
    same_time.insert("_id", doc! { "$lt": id });
    Ok(doc! {
        "$and": [
            base,
            {
                "$or": [
                    older,
                    same_time,
                ]
            }
        ]
    })
}

fn map_page<T, D, F>(
    mut docs: Vec<D>,
    limit: u32,
    mut convert: F,
) -> Result<TriggerPage<T>, TriggerStoreError>
where
    F: FnMut(D) -> Result<(T, String), TriggerStoreError>,
{
    let has_next = docs.len() > limit as usize;
    if has_next {
        docs.truncate(limit as usize);
    }
    let mut items = Vec::with_capacity(docs.len());
    let mut cursor = None;
    for doc in docs {
        let (item, next) = convert(doc)?;
        items.push(item);
        cursor = Some(next);
    }
    Ok(TriggerPage {
        items,
        next: has_next.then_some(cursor).flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{LaunchEventKind, LaunchProgressState};
    use std::collections::VecDeque;

    struct InjectedLaunchCommitter {
        outcomes: VecDeque<LaunchCommitAttempt>,
        attempted_at: Vec<tokio::time::Instant>,
        hang_after_injections: bool,
    }

    impl InjectedLaunchCommitter {
        fn new(outcomes: impl IntoIterator<Item = LaunchCommitAttempt>) -> Self {
            Self {
                outcomes: outcomes.into_iter().collect(),
                attempted_at: Vec::new(),
                hang_after_injections: false,
            }
        }

        fn hanging() -> Self {
            Self {
                outcomes: VecDeque::new(),
                attempted_at: Vec::new(),
                hang_after_injections: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl LaunchTransactionCommitter for InjectedLaunchCommitter {
        async fn commit_once(&mut self) -> LaunchCommitAttempt {
            self.attempted_at.push(tokio::time::Instant::now());
            if let Some(outcome) = self.outcomes.pop_front() {
                return outcome;
            }
            if self.hang_after_injections {
                std::future::pending().await
            } else {
                panic!("the injected Launch commit outcomes were exhausted")
            }
        }
    }

    fn unknown_commit_result(message: &'static str) -> LaunchCommitAttempt {
        LaunchCommitAttempt::Unknown(Error::custom(message))
    }

    fn event(sequence: u64) -> LaunchEvent {
        LaunchEvent {
            sequence,
            launch_id: LaunchId("lnch_event_store".into()),
            occurred_at: "2026-08-27T00:00:00Z".into(),
            correlation_id: format!("correlation-{sequence}"),
            progress: LaunchProgressState {
                state: LaunchState::Draft,
                blocked_reason: None,
                failure: None,
                resume_state: None,
                reach: None,
                delivery: None,
            },
            kind: LaunchEventKind::DraftUpdated,
        }
    }

    fn launch_document() -> Document {
        let launch = crate::trigger::worker::tests::launch_fixture();
        mongodb::bson::serialize_to_document(&launch_doc(&launch, true).unwrap())
            .expect("Launch document serializes")
    }

    fn runtime_admission(document: &mut Document) -> &mut Document {
        document
            .get_document_mut("launch")
            .unwrap()
            .get_document_mut("runtime_admission")
            .unwrap()
    }

    fn trigger_resource_documents() -> (DraftDoc, RevisionDoc) {
        let revision = crate::trigger::worker::tests::launch_fixture().revision;
        let draft = TriggerDraft {
            id: revision.content.trigger_id.clone(),
            owner: revision.content.publisher_subject_id.clone(),
            selected_service_id: "service-schema-test".into(),
            version: 3,
            next_revision_number: revision.content.revision_number + 1,
            content: revision.content.clone(),
            created_at: "2026-08-27T00:00:00Z".into(),
            updated_at: "2026-08-27T00:01:00Z".into(),
        };
        (draft_doc(&draft).unwrap(), revision_doc(&revision).unwrap())
    }

    fn publication_candidate(
        seed: &TriggerDraft,
        base: &TriggerRevision,
        published_at: &str,
    ) -> (TriggerDraft, TriggerRevision) {
        let mut draft = seed.clone();
        draft.version += 1;
        draft.updated_at = published_at.into();
        let mut revision = base.clone();
        revision.id = TriggerRevisionId::new();
        revision.public_id = PublicTriggerId::new();
        revision.published_at = published_at.into();
        (draft, revision)
    }

    #[test]
    fn owner_session_lookup_targets_the_persisted_nested_identity() {
        let filter = launch_query_filter(&LaunchQuery {
            owner: Some(SubjectId("subject-session-owner".into())),
            public_id: None,
            session_id: Some(layer2_core::SessionId("sesn_exact_launch".into())),
            states: Vec::new(),
            updated_before: None,
            page: TriggerPageRequest::default(),
        })
        .unwrap();

        assert_eq!(filter.get_str("owner").unwrap(), "subject-session-owner");
        assert_eq!(
            filter.get_str("launch.session.session_id").unwrap(),
            "sesn_exact_launch"
        );
        assert_eq!(filter.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn launch_commit_recovers_from_injected_database_failover_with_bounded_backoff() {
        let mut committer = InjectedLaunchCommitter::new([
            unknown_commit_result("primary stepped down"),
            unknown_commit_result("new primary not selected"),
            LaunchCommitAttempt::Committed,
        ]);
        let started = tokio::time::Instant::now();

        commit_launch_transaction(&mut committer)
            .await
            .expect("a confirmed commit settles failover ambiguity");

        assert_eq!(committer.attempted_at.len(), 3);
        assert_eq!(committer.attempted_at[0], started);
        assert_eq!(
            committer.attempted_at[1].duration_since(started),
            LAUNCH_TRANSACTION_COMMIT_INITIAL_BACKOFF
        );
        assert_eq!(
            committer.attempted_at[2].duration_since(started),
            LAUNCH_TRANSACTION_COMMIT_INITIAL_BACKOFF * 3
        );
    }

    #[tokio::test(start_paused = true)]
    async fn launch_commit_preserves_persistent_unknown_result_after_the_attempt_budget() {
        let mut committer = InjectedLaunchCommitter::new(
            (0..LAUNCH_TRANSACTION_COMMIT_ATTEMPTS)
                .map(|_| unknown_commit_result("commit reply lost")),
        );
        let started = tokio::time::Instant::now();

        let result = commit_launch_transaction(&mut committer).await;

        assert!(matches!(result, Err(LaunchCommitFailure::Unknown(_))));
        assert_eq!(
            committer.attempted_at.len(),
            LAUNCH_TRANSACTION_COMMIT_ATTEMPTS
        );
        assert_eq!(
            tokio::time::Instant::now().duration_since(started),
            Duration::from_millis(375)
        );
        assert!(tokio::time::Instant::now().duration_since(started) < Duration::from_secs(120));
    }

    #[tokio::test(start_paused = true)]
    async fn launch_commit_backoff_never_crosses_the_commit_deadline() {
        let mut committer = InjectedLaunchCommitter::new([unknown_commit_result(
            "primary election still in progress",
        )]);
        let started = tokio::time::Instant::now();

        let result = commit_launch_transaction_with_policy(
            &mut committer,
            LAUNCH_TRANSACTION_COMMIT_ATTEMPTS,
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .await;

        assert!(matches!(
            result,
            Err(LaunchCommitFailure::Deadline(Some(_)))
        ));
        assert_eq!(committer.attempted_at, vec![started]);
        assert_eq!(tokio::time::Instant::now(), started);
    }

    #[tokio::test(start_paused = true)]
    async fn launch_commit_hung_failover_is_cut_off_before_the_worker_lease() {
        let mut committer = InjectedLaunchCommitter::hanging();
        let started = tokio::time::Instant::now();

        let result = commit_launch_transaction(&mut committer).await;

        assert!(matches!(result, Err(LaunchCommitFailure::Deadline(None))));
        assert_eq!(committer.attempted_at.len(), 1);
        assert_eq!(
            tokio::time::Instant::now().duration_since(started),
            LAUNCH_TRANSACTION_COMMIT_DEADLINE
        );
        assert!(LAUNCH_TRANSACTION_COMMIT_DEADLINE < Duration::from_secs(120));
    }

    #[test]
    fn cursor_round_trips_without_exposing_query_structure() {
        let cursor = encode_cursor("2026-08-26T12:00:00Z", "opaque-id");
        assert_eq!(
            decode_cursor(&cursor).unwrap(),
            ("2026-08-26T12:00:00Z", "opaque-id")
        );
        assert!(decode_cursor("not-a-cursor").is_err());
    }

    #[test]
    fn stored_counters_never_wrap_between_unsigned_core_and_signed_bson() {
        assert_eq!(signed(i64::MAX as u64, "counter").unwrap(), i64::MAX);
        assert!(signed(i64::MAX as u64 + 1, "counter").is_err());
        assert!(unsigned(-1, "counter").is_err());
    }

    #[test]
    fn trigger_resource_documents_write_current_schema_and_read_legacy_schema() {
        let (draft, revision) = trigger_resource_documents();
        assert_eq!(
            draft.document_schema_version,
            TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(
            revision.document_schema_version,
            TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION
        );

        let mut legacy_draft = mongodb::bson::serialize_to_document(&draft).unwrap();
        legacy_draft.remove("document_schema_version");
        let legacy_draft =
            mongodb::bson::deserialize_from_document::<DraftDoc>(legacy_draft).unwrap();
        assert_eq!(legacy_draft.document_schema_version, 0);
        assert!(read_draft(legacy_draft).is_ok());

        let mut legacy_revision = mongodb::bson::serialize_to_document(&revision).unwrap();
        legacy_revision.remove("document_schema_version");
        let legacy_revision =
            mongodb::bson::deserialize_from_document::<RevisionDoc>(legacy_revision).unwrap();
        assert_eq!(legacy_revision.document_schema_version, 0);
        assert!(read_revision(legacy_revision).is_ok());
    }

    #[test]
    fn trigger_resource_documents_reject_future_schema_versions() {
        let (mut draft, mut revision) = trigger_resource_documents();
        draft.document_schema_version = TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION + 1;
        revision.document_schema_version = TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION + 1;

        assert!(matches!(
            read_draft(draft),
            Err(TriggerStoreError::Malformed(_))
        ));
        assert!(matches!(
            read_revision(revision),
            Err(TriggerStoreError::Malformed(_))
        ));
    }

    #[test]
    fn legacy_schema_migration_targets_only_one_exact_missing_or_zero_document() {
        assert_eq!(
            legacy_document_schema_filter(),
            doc! {
                "$or": [
                    { "document_schema_version": { "$exists": false } },
                    {
                        "$expr": {
                            "$and": [
                                {
                                    "$in": [
                                        { "$type": "$document_schema_version" },
                                        ["int", "long"],
                                    ],
                                },
                                { "$eq": ["$document_schema_version", 0_i64] },
                            ],
                        },
                    },
                ],
            }
        );
        assert_eq!(
            exact_legacy_document_schema_filter("trigger-id"),
            doc! {
                "$and": [
                    { "_id": "trigger-id" },
                    legacy_document_schema_filter(),
                ],
            }
        );
        assert_eq!(
            current_document_schema_update(1),
            doc! { "$set": { "document_schema_version": 1_i64 } }
        );
        assert!(validate_document_schema_version(0, 1, "record").is_ok());
        assert!(validate_document_schema_version(1, 1, "record").is_ok());
        assert!(validate_document_schema_version(-1, 1, "record").is_err());
        assert!(validate_document_schema_version(2, 1, "record").is_err());
    }

    #[test]
    fn missing_historical_launch_kernel_is_migrated_at_the_cma_store_boundary() {
        let mut document = launch_document();
        runtime_admission(&mut document).remove("kernel");

        let decoded: LaunchDoc = mongodb::bson::deserialize_from_document(document)
            .expect("the historical product-owned Launch remains readable");
        assert_eq!(
            decoded.launch.runtime_admission.unwrap().kernel,
            layer2_core::KernelId::new(crate::trigger::TRIGGER_KERNEL_ID)
        );
    }

    #[test]
    fn explicit_invalid_historical_launch_kernels_fail_closed() {
        for kernel in [Bson::String(String::new()), Bson::String("other".into())] {
            let mut document = launch_document();
            runtime_admission(&mut document).insert("kernel", kernel);

            assert!(mongodb::bson::deserialize_from_document::<LaunchDoc>(document).is_err());
        }
    }

    #[test]
    fn aggregate_event_tail_is_bounded_without_changing_sequences() {
        let mut events = (1..=2_100).map(event).collect::<Vec<_>>();
        trim_event_tail(&mut events);

        assert_eq!(events.len(), LAUNCH_EVENT_TAIL_LIMIT);
        assert_eq!(events.first().unwrap().sequence, 1_589);
        assert_eq!(events.last().unwrap().sequence, 2_100);
    }

    #[test]
    fn canonical_event_identity_is_stable_and_query_fields_fail_closed() {
        let event = event(42);
        let doc = launch_event_doc(&event).unwrap();
        assert_eq!(doc.id, "lnch_event_store:00000000000000000042");
        assert_eq!(read_launch_event(doc.clone()).unwrap(), event);

        let mut malformed = doc;
        malformed.correlation_id = "different-correlation".into();
        assert!(matches!(
            read_launch_event(malformed),
            Err(TriggerStoreError::Malformed(_))
        ));
    }

    #[test]
    fn cursor_filter_uses_the_requested_time_field_and_stable_id_tiebreaker() {
        let filter = page_filter(
            doc! { "owner": "subject" },
            Some("2026-08-26T12:00:00Z|opaque-id"),
            "published_at",
        )
        .unwrap();

        assert_eq!(
            filter,
            doc! {
                "$and": [
                    { "owner": "subject" },
                    {
                        "$or": [
                            { "published_at": { "$lt": "2026-08-26T12:00:00Z" } },
                            {
                                "published_at": "2026-08-26T12:00:00Z",
                                "_id": { "$lt": "opaque-id" },
                            },
                        ],
                    },
                ],
            }
        );
        assert!(!format!("{filter:?}").contains("time_field"));
    }

    #[test]
    fn settlement_filters_include_launches_superseded_as_click_targets() {
        let states = Bson::Array(vec![Bson::String("preflight".into())]);
        let terminal_states = Bson::Array(vec![Bson::String("cancelled".into())]);
        let filters = [
            connection_revocation_launch_filter(&SubjectId("owner".into()), "service-1"),
            worker_candidate_filter(states.clone(), Some("2026-08-27T00:00:00Z")),
            capability_candidate_filter(
                states.clone(),
                Bson::String("revoked".into()),
                terminal_states,
                Bson::String("cancelled".into()),
                Bson::Array(vec![
                    Bson::String("completed".into()),
                    Bson::String("failed".into()),
                ]),
            ),
            cleanup_candidate_filter(states, "2026-08-27T00:00:00Z"),
        ];

        for filter in filters {
            assert!(
                !filter.contains_key("current"),
                "settlement inventory must not use the README-click selector: {filter:?}"
            );
        }
    }

    #[test]
    fn capability_inventory_retains_unsettled_cancelled_sessions() {
        let filter = capability_candidate_filter(
            Bson::Array(vec![Bson::String("cancelled".into())]),
            Bson::String("revoked".into()),
            Bson::Array(vec![Bson::String("cancelled".into())]),
            Bson::String("cancelled".into()),
            Bson::Array(vec![
                Bson::String("completed".into()),
                Bson::String("failed".into()),
            ]),
        );
        let shape = format!("{filter:?}");

        assert!(shape.contains("launch.session"));
        assert!(shape.contains("launch.delivery.state"));
        assert!(shape.contains("$nin"));
        assert!(shape.contains("completed"));
        assert!(shape.contains("failed"));
    }

    #[test]
    fn worker_inventory_rate_bounds_open_pull_request_status_reads() {
        let filter = worker_candidate_filter(
            Bson::Array(vec![Bson::String("pr_open".into())]),
            Some("2026-08-27T00:00:00Z"),
        );
        let shape = format!("{filter:?}");

        assert!(shape.contains("launch.pull_request.state"));
        assert!(shape.contains("launch.pull_request.status_sync.next_attempt_at"));
        assert!(shape.contains("$lte"));
        assert!(shape.contains("$exists"));
        assert!(shape.contains("pr_open"));
    }

    #[test]
    fn cleanup_inventory_includes_terminal_staging_only_imports() {
        let filter = cleanup_candidate_filter(
            Bson::Array(vec![Bson::String("cancelled".into())]),
            "2026-08-27T00:00:00Z",
        );
        let shape = format!("{filter:?}");

        assert!(shape.contains("launch.workspace.delete_after"));
        assert!(shape.contains("launch.terminal_at"));
        assert!(shape.contains("$elemMatch"));
        assert!(shape.contains("workspace_import"));
    }

    #[test]
    fn exact_launch_updates_preserve_the_stored_click_selector() {
        let update = preserve_current_selector(doc! {
            "_id": "lnch_superseded",
            "current": false,
            "state": "preflight",
            "revision_number": 7_i64,
        });

        assert_eq!(
            update,
            doc! {
                "$set": {
                    "state": "preflight",
                    "revision_number": 7_i64,
                },
            }
        );
    }

    #[tokio::test]
    async fn authority_collections_use_majority_writes() {
        let client = Client::with_uri_str("mongodb://127.0.0.1:27017")
            .await
            .expect("URI parses without opening a connection");
        let database = client.database("test");
        for collection in [
            majority_collection::<DraftDoc>(&database, TRIGGER_DRAFTS),
            majority_collection::<DraftDoc>(&database, TRIGGER_REVISIONS),
            majority_collection::<DraftDoc>(&database, TRIGGER_LAUNCHES),
            majority_collection::<DraftDoc>(&database, TRIGGER_LAUNCH_EVENTS),
            majority_collection::<DraftDoc>(&database, TRIGGER_CONNECTION_REVOCATIONS),
        ] {
            assert!(collection.write_concern().is_some());
        }
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
    async fn real_mongo_lazily_migrates_exact_trigger_resources_and_fences_mutations() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("connect to MongoDB");
        let database_name = format!(
            "cma_trigger_schema_{}",
            mongodb::bson::oid::ObjectId::new().to_hex()
        );
        let database = client.database(&database_name);
        let drafts = database.collection::<Document>(TRIGGER_DRAFTS);
        let revisions = database.collection::<Document>(TRIGGER_REVISIONS);
        let (draft, revision) = trigger_resource_documents();
        let draft_id = draft.id.clone();
        let revision_id = revision.id.clone();
        let malformed_id = format!("malformed-{}", mongodb::bson::oid::ObjectId::new().to_hex());

        let mut legacy_draft = mongodb::bson::serialize_to_document(&draft).unwrap();
        legacy_draft.remove("document_schema_version");
        let mut malformed = legacy_draft.clone();
        malformed.insert("_id", &malformed_id);
        malformed.insert(
            "document_schema_version",
            Bson::Array(vec![Bson::Int64(0), Bson::Int64(2)]),
        );
        drafts
            .insert_one(legacy_draft)
            .await
            .expect("insert legacy draft");
        let mut legacy_revision = mongodb::bson::serialize_to_document(&revision).unwrap();
        legacy_revision.insert("document_schema_version", 0_i64);
        revisions
            .insert_one(legacy_revision)
            .await
            .expect("insert legacy revision");
        drafts
            .insert_one(malformed)
            .await
            .expect("insert malformed schema evidence");

        let store = MongoTriggerStore::open(&client, &database_name)
            .await
            .expect("open Trigger store without a collection scan");

        let unopened_draft = drafts
            .find_one(doc! { "_id": &draft_id })
            .await
            .unwrap()
            .unwrap();
        let unopened_revision = revisions
            .find_one(doc! { "_id": &revision_id })
            .await
            .unwrap()
            .unwrap();
        assert!(!unopened_draft.contains_key("document_schema_version"));
        assert_eq!(
            unopened_revision
                .get_i64("document_schema_version")
                .unwrap(),
            0
        );

        assert_eq!(
            store
                .get_draft(&TriggerId(draft_id.clone()))
                .await
                .unwrap()
                .unwrap(),
            draft.draft
        );
        assert_eq!(
            store
                .get_revision(&TriggerRevisionId(revision_id.clone()))
                .await
                .unwrap()
                .unwrap(),
            revision.revision
        );

        let migrated_draft = drafts
            .find_one(doc! { "_id": &draft_id })
            .await
            .unwrap()
            .unwrap();
        let migrated_revision = revisions
            .find_one(doc! { "_id": &revision_id })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            migrated_draft.get_i64("document_schema_version").unwrap(),
            TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(
            migrated_revision
                .get_i64("document_schema_version")
                .unwrap(),
            TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(
            drafts
                .find_one(doc! { "_id": &malformed_id })
                .await
                .unwrap()
                .unwrap()
                .get_array("document_schema_version")
                .unwrap(),
            &vec![Bson::Int64(0), Bson::Int64(2)]
        );
        assert!(matches!(
            store.get_draft(&TriggerId(malformed_id.clone())).await,
            Err(TriggerStoreError::Malformed(_))
        ));

        let mut legacy_mutation = draft.draft.clone();
        legacy_mutation.id = TriggerId::new();
        let legacy_mutation_id = legacy_mutation.id.0.clone();
        let mut legacy_mutation_document =
            mongodb::bson::serialize_to_document(&draft_doc(&legacy_mutation).unwrap()).unwrap();
        legacy_mutation_document.remove("document_schema_version");
        drafts
            .insert_one(legacy_mutation_document)
            .await
            .expect("insert legacy mutation draft");
        let mut replacement = legacy_mutation.clone();
        replacement.version += 1;
        assert_eq!(
            store
                .replace_draft(legacy_mutation.version, &replacement)
                .await
                .expect("an exact legacy draft is migrated before replacement"),
            replacement
        );
        assert_eq!(
            drafts
                .find_one(doc! { "_id": &legacy_mutation_id })
                .await
                .unwrap()
                .unwrap()
                .get_i64("document_schema_version")
                .unwrap(),
            TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION
        );

        let mut legacy_retirement = revision.revision.clone();
        legacy_retirement.id = TriggerRevisionId::new();
        legacy_retirement.public_id = PublicTriggerId::new();
        let mut legacy_retirement_document =
            mongodb::bson::serialize_to_document(&revision_doc(&legacy_retirement).unwrap())
                .unwrap();
        legacy_retirement_document.insert("document_schema_version", 0_i64);
        revisions
            .insert_one(legacy_retirement_document)
            .await
            .expect("insert legacy retirement revision");
        let retired = store
            .retire_revision(
                &legacy_retirement.id,
                &legacy_retirement.content.publisher_subject_id,
                "2026-08-28T00:10:00Z",
            )
            .await
            .expect("legacy revision retirement is migrated")
            .expect("revision exists");
        assert_eq!(
            retired.availability,
            layer2_core::TriggerAvailability::Retired
        );
        assert_eq!(retired.retired_at.as_deref(), Some("2026-08-28T00:10:00Z"));

        let mut future_draft = draft.draft.clone();
        future_draft.id = TriggerId::new();
        let future_draft_id = future_draft.id.0.clone();
        let mut future_draft_document =
            mongodb::bson::serialize_to_document(&draft_doc(&future_draft).unwrap()).unwrap();
        future_draft_document.insert(
            "document_schema_version",
            TRIGGER_DRAFT_DOCUMENT_SCHEMA_VERSION + 1,
        );
        drafts
            .insert_one(future_draft_document)
            .await
            .expect("insert future-schema draft");
        let mut future_replacement = future_draft.clone();
        future_replacement.version += 1;
        assert!(matches!(
            store
                .replace_draft(future_draft.version, &future_replacement)
                .await,
            Err(TriggerStoreError::Malformed(_))
        ));
        assert_eq!(
            drafts
                .find_one(doc! { "_id": &future_draft_id })
                .await
                .unwrap()
                .unwrap()
                .get_i64("version")
                .unwrap(),
            signed(future_draft.version, "future draft version").unwrap()
        );

        let (_, future_publication_revision) =
            publication_candidate(&future_draft, &revision.revision, "2026-08-28T00:11:00Z");
        assert!(matches!(
            store
                .publish_revision(
                    future_draft.version,
                    &future_replacement,
                    &future_publication_revision,
                )
                .await,
            Err(TriggerStoreError::Malformed(_))
        ));
        assert!(revisions
            .find_one(doc! { "_id": &future_publication_revision.id.0 })
            .await
            .unwrap()
            .is_none());

        let mut future_revision = revision.revision.clone();
        future_revision.id = TriggerRevisionId::new();
        future_revision.public_id = PublicTriggerId::new();
        let mut future_revision_document =
            mongodb::bson::serialize_to_document(&revision_doc(&future_revision).unwrap()).unwrap();
        future_revision_document.insert(
            "document_schema_version",
            TRIGGER_REVISION_DOCUMENT_SCHEMA_VERSION + 1,
        );
        revisions
            .insert_one(future_revision_document)
            .await
            .expect("insert future-schema revision");
        assert!(matches!(
            store
                .retire_revision(
                    &future_revision.id,
                    &future_revision.content.publisher_subject_id,
                    "2026-08-28T00:12:00Z",
                )
                .await,
            Err(TriggerStoreError::Malformed(_))
        ));
        assert_eq!(
            revisions
                .find_one(doc! { "_id": &future_revision.id.0 })
                .await
                .unwrap()
                .unwrap()
                .get_document("revision")
                .unwrap()
                .get_str("availability")
                .unwrap(),
            "published"
        );

        database.drop().await.expect("drop disposable database");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn real_mongo_concurrent_publication_retries_the_loser_to_revision_conflict() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("connect to MongoDB");
        let database_name = format!(
            "cma_trigger_publish_race_{}",
            mongodb::bson::oid::ObjectId::new().to_hex()
        );
        let store = MongoTriggerStore::open(&client, &database_name)
            .await
            .expect("open Trigger store");
        let (draft, revision) = trigger_resource_documents();
        let seed = draft.draft;
        store.insert_draft(&seed).await.expect("insert race seed");
        let expected = seed.version;
        let (left_draft, left_revision) =
            publication_candidate(&seed, &revision.revision, "2026-08-28T00:20:00Z");
        let (right_draft, right_revision) =
            publication_candidate(&seed, &revision.revision, "2026-08-28T00:20:01Z");
        let barrier = tokio::sync::Barrier::new(2);

        let (left, right) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                store.publish_revision_transaction(
                    expected,
                    &left_draft,
                    &left_revision,
                    Some(&barrier),
                ),
                store.publish_revision_transaction(
                    expected,
                    &right_draft,
                    &right_revision,
                    Some(&barrier),
                ),
            )
        })
        .await
        .expect("the synchronized publication race must not hang");

        let mut successes = 0;
        let mut conflicts = 0;
        for outcome in [left, right] {
            match outcome {
                Ok(_) => successes += 1,
                Err(TriggerStoreError::RevisionConflict {
                    expected: seen,
                    actual,
                }) => {
                    assert_eq!(seen, expected);
                    assert_eq!(actual, expected + 1);
                    conflicts += 1;
                }
                Err(error) => {
                    panic!("publication race returned a non-deterministic error: {error}")
                }
            }
        }
        assert_eq!(successes, 1);
        assert_eq!(conflicts, 1);
        assert_eq!(
            store
                .list_revisions_for(&seed.owner, &TriggerPageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            1
        );

        client
            .database(&database_name)
            .drop()
            .await
            .expect("drop disposable database");
    }
}

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
use mongodb::error::{Error, ErrorKind, WriteFailure, UNKNOWN_TRANSACTION_COMMIT_RESULT};
use mongodb::options::{IndexOptions, ReturnDocument};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

use super::durability::majority_collection;

const TRIGGER_DRAFTS: &str = "trigger_drafts";
const TRIGGER_REVISIONS: &str = "trigger_revisions";
const TRIGGER_LAUNCHES: &str = "trigger_launches";
const TRIGGER_LAUNCH_EVENTS: &str = "trigger_launch_events";
const TRIGGER_CONNECTION_REVOCATIONS: &str = "trigger_connection_revocations";
const DUPLICATE_KEY: i32 = 11_000;
const CONNECTION_REVOCATION_CAS_RETRIES: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DraftDoc {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    version: i64,
    updated_at: String,
    draft: TriggerDraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RevisionDoc {
    #[serde(rename = "_id")]
    id: String,
    public_id: String,
    owner: String,
    published_at: String,
    revision: TriggerRevision,
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
    launch: Launch,
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
        self.drafts
            .find_one(doc! { "_id": id })
            .projection(doc! { "version": 1 })
            .await
            .map_err(|error| unavailable("read Trigger draft revision", error))?
            .map(|doc| unsigned(doc.version, "Trigger draft revision"))
            .transpose()
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
        self.drafts
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| unavailable("read Trigger draft", error))?
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
            .map_err(|error| unavailable("read Trigger draft page", error))?;
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
        let result = self
            .drafts
            .find_one_and_replace(
                doc! { "_id": &draft.id.0, "owner": &draft.owner.0, "version": expected },
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
        let expected = signed(expected_draft_version, "Trigger draft expected revision")?;
        let draft_doc = draft_doc(draft)?;
        let revision_doc = revision_doc(revision)?;
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start Trigger publication transaction", error))?;
        session
            .start_transaction()
            .await
            .map_err(|error| unavailable("begin Trigger publication transaction", error))?;
        let replaced = self
            .drafts
            .replace_one(
                doc! { "_id": &draft.id.0, "owner": &draft.owner.0, "version": expected },
                &draft_doc,
            )
            .session(&mut session)
            .await;
        let replaced = match replaced {
            Ok(result) => result,
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
            return Err(if is_duplicate_key(&error) {
                TriggerStoreError::AlreadyExists
            } else {
                unavailable("insert immutable Trigger revision", error)
            });
        }
        commit(&mut session, "commit Trigger publication").await?;
        Ok(PublishedRevision {
            draft: read_draft(draft_doc)?,
            revision: read_revision(revision_doc)?,
        })
    }

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.revisions
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| unavailable("read Trigger revision", error))?
            .map(read_revision)
            .transpose()
    }

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.revisions
            .find_one(doc! { "public_id": &id.0 })
            .await
            .map_err(|error| unavailable("read public Trigger revision", error))?
            .map(read_revision)
            .transpose()
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
            .map_err(|error| unavailable("read Trigger revision page", error))?;
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
        let updated = self
            .revisions
            .find_one_and_update(
                doc! {
                    "_id": &id.0,
                    "owner": &owner.0,
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
            .map_err(|error| unavailable("reconcile retired Trigger revision", error))?
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
        let mut filter = Document::new();
        if let Some(owner) = query.owner.as_ref() {
            filter.insert("owner", owner.0.clone());
        }
        if let Some(public_id) = query.public_id.as_ref() {
            filter.insert("launch.revision.public_id", public_id.0.clone());
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
        let filter = page_filter(
            capability_candidate_filter(states, revoked, terminal_states),
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
    let mut filter = doc! { "state": { "$in": states } };
    if let Some(bound) = updated_before {
        filter.insert("updated_at", doc! { "$lt": bound });
    }
    filter
}

fn capability_candidate_filter(states: Bson, revoked: Bson, terminal_states: Bson) -> Document {
    doc! {
        "state": { "$in": states },
        "$or": [
            { "launch.reach.state": { "$exists": true, "$ne": revoked } },
            {
                "state": { "$in": terminal_states },
                "launch.session_credential_retirement": { "$exists": false },
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
        owner: draft.owner.0.clone(),
        version: signed(draft.version, "Trigger draft revision")?,
        updated_at: draft.updated_at.clone(),
        draft: draft.clone(),
    })
}

fn read_draft(doc: DraftDoc) -> Result<TriggerDraft, TriggerStoreError> {
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
        public_id: revision.public_id.0.clone(),
        owner: revision.content.publisher_subject_id.0.clone(),
        published_at: revision.published_at.clone(),
        revision: revision.clone(),
    })
}

fn read_revision(doc: RevisionDoc) -> Result<TriggerRevision, TriggerStoreError> {
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

async fn commit(
    session: &mut mongodb::ClientSession,
    context: &str,
) -> Result<(), TriggerStoreError> {
    loop {
        match session.commit_transaction().await {
            Ok(()) => return Ok(()),
            Err(error) if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) => continue,
            Err(error) => return Err(unavailable(context, error)),
        }
    }
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
}

//! Trigger drafts, immutable revisions, and fenced Launch aggregates in MongoDB.

use futures_util::TryStreamExt as _;
use layer2_core::{
    Launch, LaunchId, LaunchQuery, LaunchState, LaunchStore, PublicTriggerId, PublishedRevision,
    SubjectId, TriggerDraft, TriggerId, TriggerPage, TriggerPageRequest, TriggerRevision,
    TriggerRevisionId, TriggerStore, TriggerStoreError, WorkerLease,
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
const DUPLICATE_KEY: i32 = 11_000;

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
    launch: Launch,
}

#[derive(Clone)]
pub struct MongoTriggerStore {
    client: Client,
    drafts: Collection<DraftDoc>,
    revisions: Collection<RevisionDoc>,
    launches: Collection<LaunchDoc>,
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
                .keys(doc! { "state": 1, "lease_expires_at": 1, "updated_at": 1 })
                .options(named("trigger_launches_runnable"))
                .build(),
        ] {
            self.launches
                .create_index(index)
                .await
                .map_err(|error| unavailable("create Launch index", error))?;
        }
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
        if !doc.current {
            return Ok(TriggerStoreError::Superseded);
        }
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
        match self.launches.insert_one(launch_doc(launch, true)?).await {
            Ok(_) => Ok(launch.clone()),
            Err(error) if is_duplicate_key(&error) => self
                .launches
                .find_one(doc! {
                    "owner": &launch.owner.0,
                    "revision_id": &launch.revision.id.0,
                    "current": true,
                })
                .await
                .map_err(|read| unavailable("reconcile current Launch", read))?
                .map(read_launch)
                .transpose()?
                .ok_or(TriggerStoreError::AlreadyExists),
            Err(error) => Err(unavailable("insert Launch", error)),
        }
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
        commit(&mut session, "commit new Launch").await
    }

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
        self.launches
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| unavailable("read Launch", error))?
            .map(read_launch)
            .transpose()
    }

    async fn list_launches(
        &self,
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let mut filter = Document::new();
        if let Some(owner) = query.owner.as_ref() {
            filter.insert("owner", owner.0.clone());
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
            "current": true,
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
        let replacement = launch_doc(launch, true)?;
        let updated = self
            .launches
            .find_one_and_replace(filter, replacement)
            .return_document(ReturnDocument::After)
            .await
            .map_err(|error| unavailable("replace Launch", error))?;
        if let Some(doc) = updated {
            return read_launch(doc);
        }
        Err(self
            .launch_after_failed_cas(&launch.id, expected_revision, expected_fence)
            .await?)
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
                    "current": true,
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
                    "current": true,
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
        launch: launch.clone(),
    })
}

fn read_launch(doc: LaunchDoc) -> Result<Launch, TriggerStoreError> {
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
        ] {
            assert!(collection.write_concern().is_some());
        }
    }
}

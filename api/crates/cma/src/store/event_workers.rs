//! Atomic, majority-durable Event Worker Profile revisions.
use super::{
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, read_record, record_bson, start_transaction,
        CommitOutcome,
    },
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::{
    bots::BotError,
    event_workers::{EventWorkerProfile, EventWorkerProfileStore},
    SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Client, Collection, IndexModel,
};

pub(crate) struct MongoEventWorkerProfileStore {
    client: Client,
    records: Collection<Document>,
    revisions: Collection<Document>,
}
impl MongoEventWorkerProfileStore {
    pub(crate) async fn open(client: &Client, name: &str) -> Result<Self, BotError> {
        let db = client.database(name);
        let store = Self {
            client: client.clone(),
            records: majority_collection(&db, "event_worker_profiles"),
            revisions: majority_collection(&db, "event_worker_profile_revisions"),
        };
        store
            .records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"owner":1,"retired":1,"_id":1})
                    .options(
                        IndexOptions::builder()
                            .name("owner_inventory".to_string())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(unavailable)?;
        Ok(store)
    }
}
fn document(row: &EventWorkerProfile) -> Result<Document, BotError> {
    row.validate()?;
    Ok(
        doc! {"_id":&row.id,"document_schema_version":super::schema::DOCUMENT_SCHEMA_VERSION,"owner":row.owner.as_str(),"revision":i64::from(row.revision),"retired":row.retired_at.is_some(),"record":record_bson(row).map_err(unavailable)?},
    )
}
fn decode(row: &Document, historical: bool) -> Result<EventWorkerProfile, BotError> {
    let value: EventWorkerProfile = read_record(row).map_err(unavailable)?;
    value.validate()?;
    let key = if historical {
        format!("{}:{}", value.id, value.revision)
    } else {
        value.id.clone()
    };
    if row.get_str("_id").ok() != Some(key.as_str())
        || row.get_bool("retired").ok() != Some(value.retired_at.is_some())
        || row.get_str("owner").ok() != Some(value.owner.as_str())
        || row.get_i64("revision").ok() != Some(i64::from(value.revision))
    {
        return Err(BotError::Unavailable);
    }
    Ok(value)
}
#[async_trait::async_trait]
impl EventWorkerProfileStore for MongoEventWorkerProfileStore {
    async fn get(
        &self,
        id: &str,
        revision: Option<u32>,
    ) -> Result<Option<EventWorkerProfile>, BotError> {
        let (collection, key) = match revision {
            Some(n) => (&self.revisions, format!("{id}:{n}")),
            None => (&self.records, id.into()),
        };
        collection
            .find_one(doc! {"_id":key})
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(|row| decode(row, revision.is_some()))
            .transpose()
    }
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError> {
        let mut query = doc! {"owner":owner.as_str(),"retired":false};
        if let Some(id) = after {
            query.insert("_id", doc! {"$gt":id});
        }
        let mut cursor = self
            .records
            .find(query)
            .sort(doc! {"_id":1})
            .limit(limit.min(200) as i64)
            .await
            .map_err(unavailable)?;
        let mut rows = vec![];
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            rows.push(decode(&row, false)?);
        }
        Ok(rows)
    }
    async fn save(
        &self,
        expected: Option<u32>,
        value: &EventWorkerProfile,
    ) -> Result<(), BotError> {
        if expected.map_or(value.revision != 1, |n| {
            n.checked_add(1) != Some(value.revision)
        }) {
            return Err(BotError::Conflict);
        }
        let row = document(value)?;
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let retiring_existing = if let Some(revision) =
            expected.filter(|_| value.retired_at.is_some())
        {
            let previous = self.records.find_one(doc! {"_id":&value.id,"owner":value.owner.as_str(),"revision":i64::from(revision),"retired":false})
                .session(&mut session).await.map_err(unavailable)?
                .as_ref().map(|row| decode(row, false)).transpose()?;
            if previous.is_none_or(|previous| {
                previous.source != value.source || previous.profiles != value.profiles
            }) {
                let _ = session.abort_guarded().await;
                return Err(BotError::Conflict);
            }
            true
        } else {
            false
        };
        if let Some(revision) = expected {
            let changed=self.records.replace_one(doc!{"_id":&value.id,"owner":value.owner.as_str(),"revision":i64::from(revision),"retired":false},row.clone()).session(&mut session).await.map_err(classify)?;
            if changed.matched_count != 1 {
                let _ = session.abort_guarded().await;
                return Err(BotError::Conflict);
            }
        } else {
            self.records
                .insert_one(row.clone())
                .session(&mut session)
                .await
                .map_err(classify)?;
        }
        if !retiring_existing {
            let database = self.client.database(&self.records.namespace().db);
            if !super::profile_pin_admission::bot_profiles(&database, &value.profiles, &mut session)
                .await
                .map_err(unavailable)?
            {
                let _ = session.abort_guarded().await;
                return Err(BotError::Conflict);
            }
        }
        let mut revision = row;
        revision.insert("_id", format!("{}:{}", value.id, value.revision));
        self.revisions
            .insert_one(revision)
            .session(&mut session)
            .await
            .map_err(classify)?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            _ => Err(BotError::Unavailable),
        }
    }
}
fn unavailable(_: impl std::fmt::Display) -> BotError {
    BotError::Unavailable
}
fn classify(e: mongodb::error::Error) -> BotError {
    if is_duplicate_key(&e) {
        BotError::Conflict
    } else {
        BotError::Unavailable
    }
}

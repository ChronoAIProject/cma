//! Small independent majority-durable owner/message metadata records.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer2_core::sandbox::message_feedback::{
    FeedbackDetails, FeedbackReasonError, FeedbackReasonRequest, MessageFeedback,
    MessageFeedbackStore, MessageVote,
};
use layer2_core::sandbox::task_experience::{TaskExperiencePolicy, TaskExperienceWriter};
use layer2_core::{sandbox::AgentReadError, AgentId, StoreError, SubjectId};
use mongodb::bson::Document;
use mongodb::{bson::doc, options::IndexOptions, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{durability::majority_collection, schema};

pub(super) const COLLECTION: &str = "agent_message_feedback";

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    owner: String,
    agent_id: String,
    sequence: i64,
    #[serde(flatten)]
    details: FeedbackDetails,
    #[serde(skip)]
    expanded: bool,
}

#[derive(Clone)]
pub(crate) struct MongoMessageFeedbackStore {
    records: Collection<Document>,
    pub(crate) writer_policy: Arc<TaskExperiencePolicy>,
}

impl MongoMessageFeedbackStore {
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        schema::initialize_collections(database, &[COLLECTION]).await?;
        let records = majority_collection(database, COLLECTION);
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"owner": 1, "agent_id": 1, "sequence": 1})
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
            )
            .await
            .map_err(|_| StoreError::Unavailable("initialize message feedback index".into()))?;
        Ok(Self {
            records,
            writer_policy: Arc::default(),
        })
    }

    async fn load(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
    ) -> Result<Option<Record>, AgentReadError> {
        let id = identity(owner, agent, sequence)?;
        let row = self
            .records
            .find_one(doc! {"_id": &id})
            .await
            .map_err(unavailable)?
            .map(decode)
            .transpose()?;
        if let Some(row) = &row {
            schema::require_current(row.document_schema_version).map_err(|_| corrupt())?;
            if row.owner != owner.as_str()
                || row.agent_id != agent.as_str()
                || u64::try_from(row.sequence).ok() != Some(sequence)
                || row.id != id
            {
                return Err(corrupt());
            }
        }
        Ok(row)
    }

    async fn replace(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        previous: Option<&Record>,
        details: FeedbackDetails,
    ) -> Result<bool, AgentReadError> {
        let row = Record {
            id: identity(owner, agent, sequence)?,
            document_schema_version: schema::DOCUMENT_SCHEMA_VERSION,
            owner: owner.to_string(),
            agent_id: agent.to_string(),
            sequence: i64::try_from(sequence).map_err(|_| corrupt())?,
            details,
            expanded: self
                .writer_policy
                .allows_writer(owner, TaskExperienceWriter::FeedbackDetails)
                || previous.is_some_and(|row| row.expanded),
        };
        let document = encode(&row)?;
        if let Some(previous) = previous {
            let version = i64::try_from(previous.details.version).map_err(|_| corrupt())?;
            let mut filter =
                doc! {"_id": &row.id, "document_schema_version": schema::DOCUMENT_SCHEMA_VERSION};
            if version == 0 {
                filter.insert(
                    "$or",
                    vec![doc! {"version": 0}, doc! {"version": {"$exists": false}}],
                );
            } else {
                filter.insert("version", version);
            }
            filter.insert(
                "vote",
                mongodb::bson::serialize_to_bson(&previous.details.vote).map_err(|_| corrupt())?,
            );
            return self
                .records
                .replace_one(filter, &document)
                .await
                .map(|result| result.matched_count == 1)
                .map_err(unavailable);
        }
        match self.records.insert_one(document).await {
            Ok(_) => Ok(true),
            Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(error) => Err(unavailable(error)),
        }
    }
}

#[async_trait]
impl MessageFeedbackStore for MongoMessageFeedbackStore {
    async fn get(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
    ) -> Result<FeedbackDetails, AgentReadError> {
        Ok(self
            .load(owner, agent, sequence)
            .await?
            .map(|row| row.details)
            .unwrap_or_default())
    }

    async fn set_reasons(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        request: &FeedbackReasonRequest,
    ) -> Result<FeedbackDetails, FeedbackReasonError> {
        let previous = self.load(owner, agent, sequence).await?;
        let current = previous
            .as_ref()
            .map(|row| row.details.clone())
            .unwrap_or_default();
        let next = current.with_reasons(request)?;
        // Exact accepted requests remain replayable after admission closes.
        if next == current {
            return Ok(current);
        }
        if !self
            .writer_policy
            .allows_writer(owner, TaskExperienceWriter::FeedbackDetails)
        {
            return Err(AgentReadError::Forbidden.into());
        }
        if self
            .replace(owner, agent, sequence, previous.as_ref(), next.clone())
            .await?
        {
            return Ok(next);
        }
        let current = self.get(owner, agent, sequence).await?;
        if current.with_reasons(request)? == current {
            return Ok(current);
        }
        Err(FeedbackReasonError::Conflict)
    }

    async fn list(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<MessageFeedback>, AgentReadError> {
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        let through = i64::try_from(through).unwrap_or(i64::MAX);
        let rows: Vec<Document> = self.records
            .find(doc! {"owner": owner.as_str(), "agent_id": agent.as_str(), "sequence": {"$gt": after, "$lte": through}, "vote": {"$in": ["up", "down"]}})
            .sort(doc! {"sequence": 1})
            .limit(i64::from(limit))
            .await
            .map_err(unavailable)?
            .try_collect().await.map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
                let row = decode(row)?;
                schema::require_current(row.document_schema_version).map_err(|_| corrupt())?;
                let sequence = u64::try_from(row.sequence).map_err(|_| corrupt())?;
                if sequence == 0
                    || row.owner != owner.as_str()
                    || row.agent_id != agent.as_str()
                    || row.id != identity(owner, agent, sequence)?
                {
                    return Err(corrupt());
                }
                Ok(MessageFeedback {
                    sequence,
                    vote: row.details.vote.ok_or_else(corrupt)?,
                })
            })
            .collect()
    }

    async fn set(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        vote: Option<MessageVote>,
    ) -> Result<(), AgentReadError> {
        for _attempt in 0..32 {
            let previous = self.load(owner, agent, sequence).await?;
            let current = previous
                .as_ref()
                .map(|row| row.details.clone())
                .unwrap_or_default();
            let next = current.with_vote(vote)?;
            // The predecessor cannot read a null vote. Closing admission must
            // neither create that shape nor delete/downgrade a retained row.
            if current != next
                && !self
                    .writer_policy
                    .allows_writer(owner, TaskExperienceWriter::FeedbackDetails)
                && (vote.is_none() || previous.as_ref().is_some_and(|row| row.expanded))
            {
                return Err(AgentReadError::Forbidden);
            }
            if current == next
                || self
                    .replace(owner, agent, sequence, previous.as_ref(), next)
                    .await?
            {
                return Ok(());
            }
        }
        Err(AgentReadError::Unavailable(
            "message feedback contention; retry the same vote".into(),
        ))
    }
}

const DETAIL_FIELDS: &[&str] = &[
    "version",
    "reasons",
    "comment",
    "updated_at",
    "last_request",
];

fn decode(document: Document) -> Result<Record, AgentReadError> {
    let expanded = DETAIL_FIELDS
        .iter()
        .any(|field| document.contains_key(*field));
    let mut row: Record =
        mongodb::bson::deserialize_from_document(document).map_err(|_| corrupt())?;
    row.expanded = expanded;
    Ok(row)
}

fn encode(row: &Record) -> Result<Document, AgentReadError> {
    let mut document = mongodb::bson::serialize_to_document(row).map_err(|_| corrupt())?;
    if !row.expanded {
        for field in DETAIL_FIELDS {
            document.remove(*field);
        }
    }
    Ok(document)
}

fn identity(owner: &SubjectId, agent: &AgentId, sequence: u64) -> Result<String, AgentReadError> {
    layer2_core::profiles::content_digest(&(owner, agent, sequence)).map_err(|_| corrupt())
}
fn unavailable(_: mongodb::error::Error) -> AgentReadError {
    AgentReadError::Unavailable("message feedback storage unavailable".into())
}
fn corrupt() -> AgentReadError {
    AgentReadError::Corrupt("message feedback record is invalid".into())
}

#[cfg(test)]
#[path = "message_feedback_tests.rs"]
mod tests;

//! Small independent majority-durable owner/message metadata records.

use async_trait::async_trait;
use futures_util::TryStreamExt;
use layer2_core::sandbox::message_feedback::{MessageFeedback, MessageFeedbackStore, MessageVote};
use layer2_core::{sandbox::AgentReadError, AgentId, StoreError, SubjectId};
use mongodb::{bson::doc, options::IndexOptions, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};

use super::{durability::majority_collection, schema};

pub(super) const COLLECTION: &str = "agent_message_feedback";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    owner: String,
    agent_id: String,
    sequence: i64,
    vote: MessageVote,
}

pub(crate) struct MongoMessageFeedbackStore {
    records: Collection<Record>,
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
        Ok(Self { records })
    }
}

#[async_trait]
impl MessageFeedbackStore for MongoMessageFeedbackStore {
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
        let rows: Vec<Record> = self.records
            .find(doc! {"owner": owner.as_str(), "agent_id": agent.as_str(), "sequence": {"$gt": after, "$lte": through}})
            .sort(doc! {"sequence": 1})
            .limit(i64::from(limit))
            .await
            .map_err(unavailable)?
            .try_collect().await.map_err(unavailable)?;
        rows.into_iter()
            .map(|row| {
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
                    vote: row.vote,
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
        let id = identity(owner, agent, sequence)?;
        let sequence = i64::try_from(sequence).map_err(|_| corrupt())?;
        let filter = doc! {"_id": &id, "owner": owner.as_str(), "agent_id": agent.as_str(), "sequence": sequence};
        match vote {
            None => {
                self.records.delete_one(filter).await.map_err(unavailable)?;
            }
            Some(vote) => {
                let row = Record {
                    id,
                    document_schema_version: schema::DOCUMENT_SCHEMA_VERSION,
                    owner: owner.to_string(),
                    agent_id: agent.to_string(),
                    sequence,
                    vote,
                };
                match self
                    .records
                    .replace_one(filter.clone(), &row)
                    .upsert(true)
                    .await
                {
                    Ok(_) => {}
                    // Two first votes may both observe absence. The unique index
                    // chooses one insert; the other remains an ordinary replacement.
                    Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => {
                        self.records
                            .replace_one(filter, &row)
                            .upsert(true)
                            .await
                            .map_err(unavailable)?;
                    }
                    Err(error) => return Err(unavailable(error)),
                }
            }
        }
        Ok(())
    }
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
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires disposable Mongo fixture"]
    async fn message_feedback_survives_reopen_switches_clears_and_isolates_owner_and_agent() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI");
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let database = client.database(&format!("cma_feedback_{}", uuid::Uuid::new_v4().simple()));
        let owner = SubjectId::new("feedback-owner");
        let other = SubjectId::new("feedback-other");
        let agent = AgentId("agt_feedback".into());
        let foreign = AgentId("agt_foreign".into());
        let store = MongoMessageFeedbackStore::open(&database).await.unwrap();
        store
            .set(&owner, &agent, 4, Some(MessageVote::Up))
            .await
            .unwrap();
        store
            .set(&owner, &agent, 8, Some(MessageVote::Down))
            .await
            .unwrap();
        store
            .set(&other, &agent, 4, Some(MessageVote::Down))
            .await
            .unwrap();
        store
            .set(&owner, &foreign, 4, Some(MessageVote::Down))
            .await
            .unwrap();
        drop(store);
        let store = MongoMessageFeedbackStore::open(&database).await.unwrap();
        assert_eq!(
            store.list(&owner, &agent, 0, 8, 10).await.unwrap(),
            vec![
                MessageFeedback {
                    sequence: 4,
                    vote: MessageVote::Up
                },
                MessageFeedback {
                    sequence: 8,
                    vote: MessageVote::Down
                },
            ]
        );
        assert_eq!(store.list(&owner, &agent, 4, 8, 1).await.unwrap().len(), 1);
        assert_eq!(store.list(&owner, &agent, 0, 4, 10).await.unwrap().len(), 1);
        store
            .set(&owner, &agent, 4, Some(MessageVote::Down))
            .await
            .unwrap();
        store.set(&owner, &agent, 8, None).await.unwrap();
        store.set(&owner, &agent, 8, None).await.unwrap();
        assert_eq!(
            store.list(&owner, &agent, 0, 8, 10).await.unwrap(),
            vec![MessageFeedback {
                sequence: 4,
                vote: MessageVote::Down
            }]
        );
        assert_eq!(store.list(&other, &agent, 0, 8, 10).await.unwrap().len(), 1);
        assert_eq!(
            store.list(&owner, &foreign, 0, 8, 10).await.unwrap().len(),
            1
        );
        database.drop().await.unwrap();
    }
}

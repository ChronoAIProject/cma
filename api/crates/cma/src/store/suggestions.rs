use layer2_core::sandbox::task_experience::TaskExperienceWriter;
use layer2_core::sandbox::{agent_io::AnswerContext, suggestions::*, AgentReadError};
use layer2_core::{AgentId, StoreError, SubjectId};
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    ClientSession, Collection, IndexModel,
};

use super::sandbox_mongo::{
    commit_transaction, is_confirmed_body_retry, is_transient, read_record, record_bson,
    start_transaction, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

mod claim;
mod completion;
mod context;
mod parent;
pub(super) use context::enqueue;

pub(super) async fn ensure_indexes(collection: &Collection<Document>) -> Result<(), StoreError> {
    collection
        .create_indexes([
            IndexModel::builder()
                .keys(doc! {"owner":1,"agent_id":1,"answer_sequence":1,"version":1})
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .name("suggestion_answer_version".to_string())
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! {"version":1,"status":1,"created_at_ms":1,"_id":1})
                .build(),
            IndexModel::builder()
                .keys(doc! {"version":1,"status":1,"deadline_ms":1})
                .build(),
        ])
        .await
        .map_err(|_| StoreError::Unavailable("initialize suggestion indexes".into()))?;
    Ok(())
}

fn unavailable(_: impl std::fmt::Debug) -> AgentReadError {
    AgentReadError::Unavailable("suggestion persistence unavailable".into())
}

pub(super) fn document(work: &SuggestionWork) -> Result<Document, AgentReadError> {
    Ok(doc! {
        "_id": &work.id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": work.owner.as_str(), "agent_id": work.agent_id.as_str(),
        "answer_sequence": work.scope.answer_sequence as i64, "version": &work.strategy_version,
        "status": mongodb::bson::serialize_to_bson(&work.status).map_err(unavailable)?,
        "created_at_ms": work.created_at_ms as i64,
        "deadline_ms": work.deadline_ms.map(|value| value as i64),
        "record": record_bson(work).map_err(unavailable)?,
    })
}

fn decode(row: &Document) -> Result<SuggestionWork, AgentReadError> {
    let work: SuggestionWork = read_record(row).map_err(unavailable)?;
    if &document(&work)? != row {
        return Err(unavailable("inconsistent suggestion identity"));
    }
    work.scope.validate().map_err(unavailable)?;
    Ok(work)
}

fn confirmed_transient(error: &mongodb::error::Error) -> bool {
    is_transient(error) && is_confirmed_body_retry(error)
}

#[async_trait::async_trait]
impl SuggestionStore for MongoSandboxStore {
    async fn read_suggestions(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        scope: AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError> {
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)?;
        let Some(row) = self.suggestions.find_one(doc! {
            "owner": owner.as_str(), "agent_id": agent.as_str(), "answer_sequence": scope.answer_sequence as i64,
            "version": STRATEGY_VERSION,
        }).session(&mut session).await.map_err(unavailable)? else {
            return Ok(SuggestionReading { status: SuggestionStatus::Unavailable, set: None, observation: None });
        };
        let work = decode(&row)?;
        if work.scope != scope {
            return Err(AgentReadError::NotFound);
        }
        Ok(work.reading(context::current(self, &work, &mut session, true).await?))
    }

    async fn claim_suggestions(
        &self,
        now_ms: u64,
    ) -> Result<Option<SuggestionWork>, AgentReadError> {
        claim::acquire(self, now_ms).await
    }

    async fn complete_suggestions(
        &self,
        claim: &SuggestionWork,
        completion: SuggestionCompletion,
    ) -> Result<(), AgentReadError> {
        completion::publish(self, claim, &completion).await
    }
}

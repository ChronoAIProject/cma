use layer2_core::sandbox::{agent_io::AnswerContext, suggestions::*, AgentReadError};
use layer2_core::{AgentId, StoreError, SubjectId};
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    ClientSession, Collection, IndexModel,
};

use super::sandbox_mongo::{
    commit_transaction, read_record, record_bson, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

mod context;
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

async fn commit(session: &mut ClientSession) -> Result<(), AgentReadError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        _ => Err(unavailable("suggestion commit not confirmed")),
    }
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
        Ok(work.reading(
            context::current(self, &work, &mut session).await?,
            unix_millis(),
        ))
    }

    async fn claim_suggestions(
        &self,
        now_ms: u64,
    ) -> Result<Option<SuggestionWork>, AgentReadError> {
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let Some(row) = self
            .suggestions
            .find_one(doc! {"version":STRATEGY_VERSION,"$or": [
                {"status":"pending"}, {"status":"running", "deadline_ms":{"$lte": now_ms as i64}}
            ]})
            .sort(doc! {"created_at_ms":1,"_id":1})
            .session(&mut session)
            .await
            .map_err(unavailable)?
        else {
            session.abort_transaction().await.map_err(unavailable)?;
            return Ok(None);
        };
        let mut work = decode(&row)?;
        let current = context::current(self, &work, &mut session).await?;
        let claimed = work.claim(current, now_ms);
        self.suggestions
            .replace_one(doc! {"_id": &work.id}, document(&work)?)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        commit(&mut session).await?;
        Ok(claimed.then_some(work))
    }

    async fn complete_suggestions(
        &self,
        claim: &SuggestionWork,
        completion: SuggestionCompletion,
    ) -> Result<(), AgentReadError> {
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let row = self
            .suggestions
            .find_one(doc! {"_id": &claim.id})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or(AgentReadError::NotFound)?;
        let mut work = decode(&row)?;
        if work.status != SuggestionStatus::Running || work.started_at_ms != claim.started_at_ms {
            session.abort_transaction().await.map_err(unavailable)?;
            return Ok(());
        }
        let current = context::current(self, &work, &mut session).await?;
        self.agents
            .update_one(
                doc! {"_id":work.agent_id.as_str()},
                doc! {"$inc":{"suggestion_completion_fence":1_i64}},
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        work.complete(current, completion);
        self.suggestions
            .replace_one(doc! {"_id": &work.id}, document(&work)?)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        commit(&mut session).await
    }
}

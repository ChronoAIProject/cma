//! Mongo adapter for durable agent-title reads and coordination.

use crate::store::agent_log_telemetry::ScanCaller;
use futures_util::TryStreamExt;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{
    Agent, AgentTitle, AgentTitleClaim, AgentTitleClaimRequest, AgentTitleSource, AgentTitleStore,
    AgentTitleStoreError, AgentTitleTranscript, SetAgentTitleOutcome, StoredAgentTitle,
};
use mongodb::bson::{doc, Document, Timestamp};
use mongodb::options::ReadConcern;

use super::agent_log::scan_complete_log_with_snapshot_session;
use super::agent_log_integrity::TransactionLogError;
use super::agent_title_claims::acquire_claim;
use super::agent_title_state::read_title_state;
use super::agent_title_writes::{complete_title, set_user_title};
use super::agents::read_agent;
use super::sandbox_mongo::MongoSandboxStore;

mod bounded;

#[async_trait::async_trait]
impl AgentTitleStore for MongoSandboxStore {
    async fn agent_title_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleStoreError> {
        title_candidates(self, page).await
    }

    async fn agent_title_transcript(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
        title_transcript(self, id).await
    }

    async fn acquire_agent_title_claim(
        &self,
        request: &AgentTitleClaimRequest,
        now: std::time::SystemTime,
    ) -> Result<Option<AgentTitleClaim>, AgentTitleStoreError> {
        acquire_claim(self, request, now).await
    }

    async fn complete_agent_title(
        &self,
        claim: &AgentTitleClaim,
        title: &AgentTitle,
        source: AgentTitleSource,
        set_at: String,
    ) -> Result<bool, AgentTitleStoreError> {
        complete_title(self, claim, title, source, set_at).await
    }

    async fn set_user_agent_title(
        &self,
        id: &AgentId,
        title: &AgentTitle,
        set_at: String,
    ) -> Result<SetAgentTitleOutcome, AgentTitleStoreError> {
        set_user_title(self, id, title, set_at).await
    }

    async fn agent_title(
        &self,
        id: &AgentId,
    ) -> Result<Option<StoredAgentTitle>, AgentTitleStoreError> {
        let document = self
            .agents
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
            .ok_or(AgentTitleStoreError::NotFound)?;
        let agent = read_agent(&document).map_err(map_store_error)?;
        Ok(read_title_state(&document, &agent)?.stored)
    }
}

async fn title_candidates(
    store: &MongoSandboxStore,
    page: &PageRequest,
) -> Result<Page<AgentId>, AgentTitleStoreError> {
    validate_page(page)?;
    let mut session = snapshot_session(store, None).await?;
    let mut filter = Document::new();
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    let mut cursor = store
        .agents
        .find(filter)
        .sort(doc! { "_id": 1 })
        .limit(i64::from(page.limit) + 1)
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut documents = Vec::new();
    while let Some(document) = cursor
        .stream(&mut session)
        .try_next()
        .await
        .map_err(unavailable)?
    {
        documents.push(document);
    }
    drop(cursor);

    let has_more = documents.len() > usize::from(page.limit);
    if has_more {
        documents.pop();
    }
    let mut items = Vec::new();
    let mut last_raw_id = None;
    for document in documents {
        let agent = read_agent(&document).map_err(map_store_error)?;
        last_raw_id = Some(agent.id.to_string());
        let title_state = read_title_state(&document, &agent)?;
        if title_state.retry.eligible(mongodb::bson::DateTime::now())
            && title_state
                .stored
                .as_ref()
                .is_none_or(|title| title.source == AgentTitleSource::Fallback)
            && transcript_at_snapshot(store, &agent, &mut session)
                .await?
                .is_some()
        {
            items.push(agent.id);
        }
    }
    Ok(Page {
        items,
        next: has_more.then_some(last_raw_id).flatten(),
    })
}

async fn title_transcript(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    let mut session = snapshot_session(store, None).await?;
    let document = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or(AgentTitleStoreError::NotFound)?;
    let agent = read_agent(&document).map_err(map_store_error)?;
    read_title_state(&document, &agent)?;
    transcript_at_snapshot(store, &agent, &mut session).await
}

pub(super) async fn transcript_at_snapshot(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    if let Some(transcript) = bounded::read(store, agent, session).await? {
        return Ok(Some(transcript));
    }
    let mut logs = Vec::new();
    scan_complete_log_with_snapshot_session(
        ScanCaller::TitleDiscovery,
        store,
        &agent.id,
        agent.conversation.last_seq,
        session,
        |entry| {
            logs.push(entry);
            Ok(())
        },
    )
    .await
    .map_err(map_log_error)?;
    Ok(AgentTitleTranscript::from_agent_log(agent, &logs))
}

pub(super) async fn snapshot_session(
    store: &MongoSandboxStore,
    snapshot_time: Option<Timestamp>,
) -> Result<mongodb::ClientSession, AgentTitleStoreError> {
    let session = store.client.start_session().snapshot(true);
    let session = match snapshot_time {
        Some(snapshot_time) => session.snapshot_time(snapshot_time),
        None => session,
    };
    session.await.map_err(unavailable)
}

fn validate_page(page: &PageRequest) -> Result<(), AgentTitleStoreError> {
    if page.limit == 0
        || page.limit > MAX_PAGE_SIZE
        || page.cursor.as_ref().is_some_and(String::is_empty)
    {
        Err(AgentTitleStoreError::InvalidPage)
    } else {
        Ok(())
    }
}

pub(super) fn map_store_error(error: SandboxStoreError) -> AgentTitleStoreError {
    match error {
        SandboxStoreError::NotFound => AgentTitleStoreError::NotFound,
        SandboxStoreError::Duplicate => AgentTitleStoreError::Duplicate,
        SandboxStoreError::SequenceConflict => AgentTitleStoreError::SequenceConflict,
        SandboxStoreError::AgentLogCapacityExceeded => {
            AgentTitleStoreError::AgentLogCapacityExceeded
        }
        SandboxStoreError::InvalidPage => AgentTitleStoreError::InvalidPage,
        SandboxStoreError::Corrupt(_) => AgentTitleStoreError::Corrupt,
        _ => AgentTitleStoreError::Unavailable,
    }
}

pub(super) fn map_log_error(error: TransactionLogError) -> AgentTitleStoreError {
    match error {
        TransactionLogError::Domain(error) => map_store_error(error),
        TransactionLogError::Driver(_) => AgentTitleStoreError::Unavailable,
    }
}

pub(super) fn unavailable(_: mongodb::error::Error) -> AgentTitleStoreError {
    AgentTitleStoreError::Unavailable
}

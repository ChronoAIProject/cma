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

use super::agent_log_integrity::{read::read_proven_agent, scan_collection, TransactionLogError};
use super::agent_title_claims::acquire_claim;
use super::agent_title_state::read_title_state;
use super::agent_title_writes::{complete_title, set_user_title};
use super::agents::read_agent;
use super::sandbox_mongo::MongoSandboxStore;

mod bounded;

impl MongoSandboxStore {
    pub(crate) fn with_title_effect_custody(mut self, enabled: bool) -> Self {
        self.title_effect_custody = enabled;
        self
    }

    pub(crate) fn with_bounded_title_reads(mut self, enabled: bool) -> Self {
        self.bounded_title_reads = enabled;
        self
    }
}

#[async_trait::async_trait]
impl AgentTitleStore for MongoSandboxStore {
    async fn agent_title_effect(
        &self,
        id: &AgentId,
    ) -> Result<layer2_core::sandbox::AgentTitleEffectState, AgentTitleStoreError> {
        super::agent_title_effect::state(self, id).await
    }
    async fn begin_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &layer2_core::sandbox::AgentTitleEffectIntent,
    ) -> Result<bool, AgentTitleStoreError> {
        super::agent_title_effect::begin(self, claim, intent).await
    }
    async fn validate_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &layer2_core::sandbox::AgentTitleEffectIntent,
    ) -> Result<bool, AgentTitleStoreError> {
        super::agent_title_effect::validate(self, claim, intent).await
    }
    async fn accept_agent_title_effect(
        &self,
        claim: &AgentTitleClaim,
        intent: &layer2_core::sandbox::AgentTitleEffectIntent,
        title: &AgentTitle,
    ) -> Result<bool, AgentTitleStoreError> {
        super::agent_title_effect::accept(self, claim, intent, title).await
    }

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
        let segmented = crate::store::agent_log_segments::read_format(&document)
            .map_err(map_store_error)?
            .is_some();
        if (!title_state.effect.unknown() || title_state.stored.is_none())
            && (title_state.effect.accepted().is_some()
                || title_state.retry.eligible(mongodb::bson::DateTime::now()))
            && title_state
                .stored
                .as_ref()
                .is_none_or(|title| title.source == AgentTitleSource::Fallback)
            && transcript_at_snapshot_with_format(store, &agent, &mut session, segmented)
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
    let segmented = crate::store::agent_log_segments::read_format(&document)
        .map_err(map_store_error)?
        .is_some();
    transcript_at_snapshot_with_format(store, &agent, &mut session, segmented).await
}

#[cfg(test)]
pub(super) async fn transcript_at_snapshot(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    let segmented = crate::store::agent_log_segment_reads::is_segmented(store, agent, session)
        .await
        .map_err(map_store_error)?;
    transcript_at_snapshot_with_format(store, agent, session, segmented).await
}

pub(super) async fn transcript_at_snapshot_with_format(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
    segmented: bool,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    if segmented {
        if let Some(transcript) = bounded::segmented_read(store, agent, session).await? {
            return Ok(Some(transcript));
        }
        // A certified segmented projection may prove that no title-eligible
        // turn exists. Do not route that valid result through the legacy
        // complete reader, which intentionally rejects format-2 metadata.
        return Ok(None);
    }
    if let Some(transcript) = bounded::read(store, agent, session).await? {
        return Ok(Some(transcript));
    }
    // Complete fallback is still a reader. Validate shared proof without the
    // explicit audit wrapper, whose corruption path persists quarantine.
    let (proven, _) = read_proven_agent(store, &agent.id, session)
        .await
        .map_err(map_store_error)?
        .ok_or(AgentTitleStoreError::NotFound)?;
    if &proven != agent {
        return Err(AgentTitleStoreError::SequenceConflict);
    }
    let mut logs = Vec::new();
    scan_collection(
        ScanCaller::TitleDiscovery,
        &store.agent_log,
        &agent.id,
        agent.conversation.last_seq,
        session,
        crate::store::agent_log_integrity::ScanOptions::new(true, store.scan_cooperation),
        |entry| {
            if entry.agent_id != agent.id {
                return Err(SandboxStoreError::SequenceConflict);
            }
            if let layer2_core::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } = &entry.kind
            {
                if entry.seq != 1
                    || workspace_id != &agent.workspace_id
                    || worktree_id != agent.creation_worktree_id()
                {
                    return Err(SandboxStoreError::Corrupt(
                        "Agent creation log lineage does not match".into(),
                    ));
                }
            }
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

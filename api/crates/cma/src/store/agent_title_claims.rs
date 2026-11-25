//! Single-writer Mongo claims for automatic agent-title generation.

use crate::store::agent_log_telemetry::ScanCaller;
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use std::time::SystemTime;

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentTitleClaim, AgentTitleClaimMode, AgentTitleClaimRequest, AgentTitleStoreError,
    AgentTitleTranscript,
};
use mongodb::bson::{doc, Bson, DateTime, Document, Timestamp};
use mongodb::options::ReadConcern;

use super::agent_log::scan_complete_log_with_session;
use super::agent_log_integrity::TransactionLogError;
use super::agent_title_state::{
    checked_claim_times, claim_fields, read_title_state, TitleState, TITLE,
};
use super::agent_titles::{map_store_error, snapshot_session};
use super::agents::read_agent;
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};

enum ApplyError {
    Domain(AgentTitleStoreError),
    Driver(mongodb::error::Error),
}

enum ClaimApply {
    Settled(Option<AgentTitleClaim>),
    Written(AgentTitleClaim),
}

pub(super) async fn acquire_claim(
    store: &MongoSandboxStore,
    request: &AgentTitleClaimRequest,
    now: SystemTime,
) -> Result<Option<AgentTitleClaim>, AgentTitleStoreError> {
    if request.holder().trim().is_empty() || request.holder().len() > 256 {
        return Err(AgentTitleStoreError::InvalidClaim);
    }
    let expires_at = now
        .checked_add(request.ttl())
        .ok_or(AgentTitleStoreError::InvalidClaim)?;
    let (now, expires_at) = checked_claim_times(now, expires_at)?;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(_) => return Err(AgentTitleStoreError::Unavailable),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(AgentTitleStoreError::Unavailable);
        }
        let claim = match apply_claim(store, request, now, expires_at, &mut session).await {
            Ok(ClaimApply::Settled(claim)) => {
                let _ = session.abort_guarded().await;
                return Ok(claim);
            }
            Ok(ClaimApply::Written(claim)) => claim,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(ApplyError::Driver(_)) => {
                let _ = session.abort_guarded().await;
                return Err(AgentTitleStoreError::Unavailable);
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(Some(claim)),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                let snapshot_time = session
                    .operation_time()
                    .ok_or(AgentTitleStoreError::Unavailable)?;
                let store = store.clone();
                return wait_for_reconciliation("agent title claim", async move {
                    reconcile_claim(&store, &claim, snapshot_time)
                        .await?
                        .then_some(claim)
                        .ok_or(AgentTitleStoreError::Unavailable)
                        .map(Some)
                })
                .await
                .map_err(|_| AgentTitleStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(_) => return Err(AgentTitleStoreError::Unavailable),
        }
    }
    Err(AgentTitleStoreError::Unavailable)
}

async fn apply_claim(
    store: &MongoSandboxStore,
    request: &AgentTitleClaimRequest,
    now: DateTime,
    expires_at: DateTime,
    session: &mut mongodb::ClientSession,
) -> Result<ClaimApply, ApplyError> {
    let Some((agent, state, server_now)) = load_agent(store, request.agent_id(), session).await?
    else {
        return Ok(ClaimApply::Settled(None));
    };
    let (now, expires_at) = if store.title_effect_custody
        || state.effect != layer2_core::sandbox::AgentTitleEffectState::Legacy
    {
        let expires = server_now
            .timestamp_millis()
            .checked_add(
                i64::try_from(request.ttl().as_millis())
                    .map_err(|_| ApplyError::Domain(AgentTitleStoreError::InvalidClaim))?,
            )
            .ok_or(ApplyError::Domain(AgentTitleStoreError::InvalidClaim))?;
        (server_now, DateTime::from_millis(expires))
    } else {
        (now, expires_at)
    };
    if agent.status == layer2_core::sandbox::AgentStatus::Deleted {
        return Ok(ClaimApply::Settled(None));
    }
    if state
        .stored
        .as_ref()
        .is_some_and(|title| title.source != layer2_core::sandbox::AgentTitleSource::Fallback)
    {
        return Ok(ClaimApply::Settled(None));
    }
    // An unresolved provider effect is never redispatched. Until its
    // prompt-derived fallback is stored, it may take a fallback-only claim.
    let unknown = state.effect.unknown();
    if unknown && state.stored.is_some() {
        return Ok(ClaimApply::Settled(None));
    }
    let accepted = state.effect.accepted().is_some();
    if !accepted && title_transcript(store, &agent, session).await?.is_none() {
        return Ok(ClaimApply::Settled(None));
    }
    if let Some(lease) = state.lease.as_ref().filter(|lease| lease.expires_at > now) {
        return Ok(ClaimApply::Settled(lease.matches_request(request).then(
            || lease.to_claim(request).with_attempts(state.retry.attempts),
        )));
    }
    if !accepted && !state.retry.eligible(now) {
        return Ok(ClaimApply::Settled(None));
    }
    let exhausted = !accepted
        && (unknown || state.retry.attempts >= layer2_core::sandbox::AGENT_TITLE_MAX_ATTEMPTS);
    let attempts = state.retry.attempts + u32::from(!exhausted && !accepted);
    let mode = if exhausted {
        AgentTitleClaimMode::FallbackOnly
    } else {
        AgentTitleClaimMode::Generate
    };
    let claim = AgentTitleClaim::acquired(
        request.clone(),
        mode,
        now.to_system_time(),
        expires_at.to_system_time(),
    )
    .with_attempts(attempts);
    let mut filter = current_agent_filter(&agent);
    filter.insert(
        TITLE,
        agent.title.clone().map(Bson::String).unwrap_or(Bson::Null),
    );
    filter.insert(
        "record.title",
        agent.title.clone().map(Bson::String).unwrap_or(Bson::Null),
    );
    let mut fields = claim_fields(&claim, state.stored.is_some());
    if store.title_effect_custody
        && state.effect == layer2_core::sandbox::AgentTitleEffectState::Legacy
    {
        fields.extend(if state.attempted {
            // Legacy attempts have no provider finality proof. Upgrading must
            // not reinterpret them as new, safely retryable obligations.
            super::agent_title_effect::legacy_unknown_fields()
        } else {
            super::agent_title_effect::initial_fields()
        });
    }
    if store.title_effect_custody
        || state.effect != layer2_core::sandbox::AgentTitleEffectState::Legacy
    {
        filter.insert("$expr", doc! {"$lt": ["$$NOW", expires_at]});
    }
    let result = store
        .agents
        .update_one(filter, doc! { "$set": fields })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(ApplyError::Domain(AgentTitleStoreError::Unavailable));
    }
    Ok(ClaimApply::Written(claim))
}

async fn load_agent(
    store: &MongoSandboxStore,
    id: &layer2_core::ids::AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<(Agent, TitleState, DateTime)>, ApplyError> {
    // Read the database clock beside the exact Agent in this transaction. A
    // caller clock jump cannot grant, extend or prematurely steal custody.
    let mut cursor = store
        .agents
        .aggregate([
            doc! {"$match": {"_id": id.as_str()}},
            doc! {"$set": {"title_server_now": "$$NOW"}},
        ])
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let document = cursor
        .stream(&mut *session)
        .try_next()
        .await
        .map_err(ApplyError::Driver)?;
    drop(cursor);
    let Some(document) = document else {
        return Ok(None);
    };
    let agent = read_agent(&document).map_err(map_domain)?;
    let state = read_title_state(&document, &agent).map_err(ApplyError::Domain)?;
    let now = document
        .get_datetime("title_server_now")
        .copied()
        .map_err(|_| ApplyError::Domain(AgentTitleStoreError::Corrupt))?;
    Ok(Some((agent, state, now)))
}

async fn title_transcript(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<Option<AgentTitleTranscript>, ApplyError> {
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| map_domain(SandboxStoreError::NotFound))?;
    if let Some(transcript) =
        super::agent_log_segments::ordinary::title(store, agent, &row, session)
            .await
            .map_err(map_transaction_log)?
    {
        return Ok(transcript);
    }
    let mut logs = Vec::new();
    scan_complete_log_with_session(
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
    .map_err(map_transaction_log)?;
    Ok(AgentTitleTranscript::from_agent_log(agent, &logs))
}

async fn reconcile_claim(
    store: &MongoSandboxStore,
    claim: &AgentTitleClaim,
    snapshot_time: Timestamp,
) -> Result<bool, AgentTitleStoreError> {
    let mut session = snapshot_session(store, Some(snapshot_time)).await?;
    let document = store
        .agents
        .find_one(doc! { "_id": claim.agent_id().as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| AgentTitleStoreError::Unavailable)?;
    let Some(document) = document else {
        return Ok(false);
    };
    let agent = read_agent(&document).map_err(map_store_error)?;
    let state = read_title_state(&document, &agent)?;
    if !state
        .lease
        .as_ref()
        .is_some_and(|lease| lease.matches_claim(claim))
    {
        return Ok(false);
    }
    let segmented = super::agent_log_segments::read_format(&document)
        .map_err(map_store_error)?
        .is_some();
    Ok(super::agent_titles::transcript_at_snapshot_with_format(
        store,
        &agent,
        &mut session,
        segmented,
    )
    .await?
    .is_some())
}

pub(super) fn current_agent_filter(agent: &Agent) -> Document {
    doc! {
        "channel_reuse_cleanup_execution": { "$exists": false },
        "_id": agent.id.as_str(),
        "document_schema_version": { "$in": [DOCUMENT_SCHEMA_VERSION, 2_i64] },
        "revision": signed(agent.revision, "agent revision").unwrap_or(i64::MIN),
        "last_seq": signed(agent.conversation.last_seq, "agent log sequence")
            .unwrap_or(i64::MIN),
        "record.conversation.last_seq": signed(
            agent.conversation.last_seq,
            "agent log sequence",
        ).unwrap_or(i64::MIN),
    }
}

fn map_domain(error: SandboxStoreError) -> ApplyError {
    ApplyError::Domain(map_store_error(error))
}

fn map_transaction_log(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Domain(error) => map_domain(error),
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
    }
}

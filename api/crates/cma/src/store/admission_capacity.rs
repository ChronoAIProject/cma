//! Counts are checked in the transaction holding the shared settings fence.
use std::time::Duration;

use layer2_core::admission::{AdmissionCause, AdmissionCeilings};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentStatus};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::agent_conversation_store::ApplyError;
use super::sandbox_mongo::MongoSandboxStore;

/// Existing active turns consume no additional slot when another message is
/// queued. Admitted starts tolerate unreadable policy; numeric policy remains
/// authoritative at the TurnStarted boundary.
pub(super) async fn require_sessions(
    store: &MongoSandboxStore,
    agent: &Agent,
    ceilings: &Result<AdmissionCeilings, ()>,
    cause: AdmissionCause,
    existing_consumer: bool,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if agent.conversation.active_turn.is_some()
        || matches!(
            agent.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        )
    {
        return Ok(());
    }
    let ceilings = match ceilings {
        Ok(ceilings) => ceilings,
        Err(()) if cause == AdmissionCause::Continuation => return Ok(()),
        Err(()) => return Err(refused()),
    };
    // Unset checks must not issue a count, cohort read or fence write.
    if ceilings.active_sessions.is_none() && ceilings.owner_active_sessions.is_none() {
        return Ok(());
    }
    if let Some(mut limit) = ceilings.active_sessions {
        let reserve = ceilings.existing_reserve.active_sessions.unwrap_or(0);
        if reserve > 0
            && !existing_consumer
            && super::registration_capacity::is_public(
                &store.sandbox_admissions,
                &agent.owner,
                Some(session),
            )
            .await
            .map_err(ApplyError::Driver)?
        {
            limit = limit.saturating_sub(reserve);
        }
        if limit == 0 || active_count(store, None, limit, session).await? >= u64::from(limit) {
            return Err(refused());
        }
    }
    if let Some(limit) = ceilings.owner_active_sessions {
        if limit == 0
            || active_count(store, Some(&agent.owner), limit, session).await? >= u64::from(limit)
        {
            return Err(refused());
        }
    }
    Ok(())
}

async fn active_count(
    store: &MongoSandboxStore,
    owner: Option<&layer2_core::SubjectId>,
    limit: u32,
    session: &mut ClientSession,
) -> Result<u64, ApplyError> {
    let mut filter = doc! {"status": {"$in": ["working", "waiting_approval"]}};
    let index = match owner {
        Some(owner) => {
            filter.insert("owner", owner.as_str());
            "agents_owner_status_updated"
        }
        None => "agents_live_connections",
    };
    store
        .agents
        .count_documents(filter)
        .hint(mongodb::options::Hint::Name(index.into()))
        .limit(u64::from(limit))
        .max_time(Duration::from_secs(3))
        .session(session)
        .await
        .map_err(ApplyError::Driver)
}

fn refused() -> ApplyError {
    ApplyError::Domain(SandboxStoreError::SessionCapacityUnavailable)
}

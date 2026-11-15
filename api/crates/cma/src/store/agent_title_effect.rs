//! One retained provider-effect barrier per Agent, independently of claim epochs.

use super::agent_title_state::{lease_filter, read_title_state};
use super::agent_titles::{map_store_error, unavailable};
use super::agents::read_agent;
use super::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::{
    AgentTitle, AgentTitleClaim, AgentTitleEffect, AgentTitleEffectIntent, AgentTitleEffectState,
    AgentTitleStoreError,
};
use layer2_core::AgentId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{
    Acknowledgment, ReadConcern, ReadPreference, SelectionCriteria, WriteConcern,
};

mod data;
pub(super) mod purge;
pub(super) use data::{initial_fields, legacy_unknown_fields, read, CUSTODY, FLOOR};

pub(super) async fn state(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<AgentTitleEffectState, AgentTitleStoreError> {
    let row = row(store, id).await?;
    let agent = read_agent(&row).map_err(map_store_error)?;
    let state = read_title_state(&row, &agent)?.effect;
    Ok(
        if state == AgentTitleEffectState::Legacy && store.title_effect_custody {
            AgentTitleEffectState::Ready
        } else {
            state
        },
    )
}

pub(super) async fn begin(
    store: &MongoSandboxStore,
    claim: &AgentTitleClaim,
    intent: &AgentTitleEffectIntent,
) -> Result<bool, AgentTitleStoreError> {
    intent.validate()?;
    if !intent.matches_claim(claim) {
        return Err(AgentTitleStoreError::InvalidClaim);
    }
    let row = row(store, claim.agent_id()).await?;
    match read(&row)? {
        AgentTitleEffectState::Ready => {}
        AgentTitleEffectState::Retained(effect)
            if effect.intent == *intent && effect.result.is_none() =>
        {
            return validate(store, claim, intent).await;
        }
        _ => return Ok(false),
    }
    let Some(mut filter) = live_filter(&row, claim)? else {
        return Ok(false);
    };
    filter.insert(CUSTODY, Bson::Null);
    let effect = AgentTitleEffect {
        version: 1,
        intent: intent.clone(),
        result: None,
    };
    let result = store
        .agents
        .update_one(
            filter,
            doc! {"$set": {FLOOR: 2_i32, CUSTODY: data::encode(&effect)?}},
        )
        .write_concern(majority())
        .await;
    if result.is_err() {
        tracing::warn!(effect = %intent.nonce,
            "title effect intent acknowledgement uncertain; checking exact retained identity"
        );
    }
    // Readback is mandatory even on success. Only the caller retaining the
    // original capability may consume it after this exact/live confirmation.
    let confirmed = validate(store, claim, intent).await?;
    if confirmed {
        tracing::info!(effect = %intent.nonce, "automatic title provider effect intent confirmed");
    }
    Ok(confirmed)
}

pub(super) async fn validate(
    store: &MongoSandboxStore,
    claim: &AgentTitleClaim,
    intent: &AgentTitleEffectIntent,
) -> Result<bool, AgentTitleStoreError> {
    intent.validate()?;
    let row = row(store, claim.agent_id()).await?;
    let AgentTitleEffectState::Retained(effect) = read(&row)? else {
        return Ok(false);
    };
    if effect.intent != *intent || effect.result.is_some() || !intent.matches_claim(claim) {
        return Ok(false);
    }
    let Some(mut filter) = live_filter(&row, claim)? else {
        return Ok(false);
    };
    filter.insert(CUSTODY, data::encode(&effect)?);
    Ok(store
        .agents
        .find_one(filter)
        .read_concern(ReadConcern::linearizable())
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .max_time(std::time::Duration::from_secs(2))
        .await
        .map_err(unavailable)?
        .is_some())
}

pub(super) async fn accept(
    store: &MongoSandboxStore,
    claim: &AgentTitleClaim,
    intent: &AgentTitleEffectIntent,
    title: &AgentTitle,
) -> Result<bool, AgentTitleStoreError> {
    intent.validate()?;
    if !intent.matches_claim(claim) {
        return Err(AgentTitleStoreError::InvalidClaim);
    }
    let row = row(store, claim.agent_id()).await?;
    let AgentTitleEffectState::Retained(mut effect) = read(&row)? else {
        return Ok(false);
    };
    if effect.intent != *intent {
        return Ok(false);
    }
    if let Some(result) = effect.result {
        return Ok(result == *title);
    }
    let Some(mut filter) = live_filter(&row, claim)? else {
        return Ok(false);
    };
    filter.insert(CUSTODY, data::encode(&effect)?);
    effect.result = Some(title.clone());
    let result = store
        .agents
        .update_one(filter, doc! {"$set": {CUSTODY: data::encode(&effect)?}})
        .write_concern(majority())
        .await;
    if result.is_err() {
        tracing::warn!(effect = %intent.nonce,
            "title effect result acknowledgement uncertain; checking exact retained identity"
        );
    }
    let stored = row_for_effect(store, claim.agent_id()).await?;
    let exact = matches!(stored, AgentTitleEffectState::Retained(stored) if stored == effect);
    if exact {
        tracing::debug!(effect = %intent.nonce, "title effect result retained; publication may proceed without redispatch");
    }
    Ok(exact)
}

async fn row_for_effect(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<AgentTitleEffectState, AgentTitleStoreError> {
    read(&row(store, id).await?)
}
async fn row(store: &MongoSandboxStore, id: &AgentId) -> Result<Document, AgentTitleStoreError> {
    // A write-concern error can precede the majority commit point. A fresh
    // majority read may still return the pre-write row. The unique _id barrier
    // waits for current primary state; absence never authorizes redispatch.
    store
        .agents
        .find_one(doc! {"_id": id.as_str()})
        .read_concern(ReadConcern::linearizable())
        .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
        .max_time(std::time::Duration::from_secs(2))
        .await
        .map_err(unavailable)?
        .ok_or(AgentTitleStoreError::NotFound)
}
fn majority() -> WriteConcern {
    WriteConcern::builder().w(Acknowledgment::Majority).build()
}
fn live_filter(
    row: &Document,
    claim: &AgentTitleClaim,
) -> Result<Option<Document>, AgentTitleStoreError> {
    let agent = read_agent(row).map_err(map_store_error)?;
    let state = read_title_state(row, &agent)?;
    if agent.id != *claim.agent_id()
        || agent.status == layer2_core::sandbox::AgentStatus::Deleted
        || state
            .stored
            .as_ref()
            .is_some_and(|title| title.source != layer2_core::sandbox::AgentTitleSource::Fallback)
        || !state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.matches_claim(claim))
    {
        return Ok(None);
    }
    let mut filter = super::agent_title_claims::current_agent_filter(&agent);
    filter.extend(lease_filter(claim, state.stored.is_some()));
    filter.insert(
        FLOOR,
        row.get(FLOOR)
            .cloned()
            .ok_or(AgentTitleStoreError::Corrupt)?,
    );
    filter.insert("title", row.get("title").cloned().unwrap_or(Bson::Null));
    // Server time gates both the original dispatch and a new result mutation.
    filter.insert(
        "$expr",
        doc! {"$lt": ["$$NOW", mongodb::bson::DateTime::from_system_time(claim.expires_at())]},
    );
    Ok(Some(filter))
}

/// Preserve any existing predicate while adding custody's server-time fence.
pub(super) fn publication_filter(
    filter: &mut Document,
    claim: &AgentTitleClaim,
    state: &super::agent_title_state::TitleState,
) {
    if state.effect != AgentTitleEffectState::Legacy {
        let existing = std::mem::take(filter);
        *filter = doc! {"$and": [existing, {"$expr": {"$lt": ["$$NOW",
        mongodb::bson::DateTime::from_system_time(claim.expires_at())]}}]};
    }
}

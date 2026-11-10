//! Transactional reservations backed by CMA's recorded model usage.

use layer2_core::budget::{BudgetError, BudgetPolicy, OwnerBudget, TurnReservation};
use layer2_core::sandbox::{store::AgentConversationMutation, Agent, AgentLogKind};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;
use serde::{Deserialize, Serialize};

use super::agent_conversation_store::ApplyError;
use super::sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

mod baseline;
mod cancellation;
mod requests;
mod source;
pub(super) use cancellation::release_unexecuted;

pub(super) const COLLECTION: &str = "model_budget_reservations";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRecord {
    owner: layer2_core::SubjectId,
    agent_id: layer2_core::AgentId,
    turn_sequence: Option<u64>,
    runtime_generation: Option<u64>,
    reservation: TurnReservation,
}

fn domain(reason: BudgetError) -> ApplyError {
    ApplyError::Domain(layer2_core::sandbox::store::SandboxStoreError::Budget(
        reason,
    ))
}

fn decode<T: serde::de::DeserializeOwned>(document: &Document) -> Result<T, ApplyError> {
    super::sandbox_mongo::read_record(document).map_err(|_| domain(BudgetError::Unavailable))
}

async fn save<T: Serialize>(
    store: &MongoSandboxStore,
    key: &str,
    record: &T,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let record =
        mongodb::bson::serialize_to_bson(record).map_err(|_| domain(BudgetError::Unavailable))?;
    store
        .model_budget
        .replace_one(
            doc! {"_id": key},
            doc! {
                "_id": key, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": record,
            },
        )
        .upsert(true)
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok(())
}

/// Initialize outside admission transactions: concurrent first admissions must
/// not contend on implicit collection creation or the singleton's first insert.
pub(super) async fn initialize_settings(
    store: &MongoSandboxStore,
) -> Result<(), layer2_core::StoreError> {
    let settings = mongodb::bson::serialize_to_bson(&layer2_core::PlatformSettings::default())
        .map_err(|_| layer2_core::StoreError::Malformed("default budget settings".into()))?;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$setOnInsert": {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": 0_i64,
                "settings": settings
            }},
        )
        .upsert(true)
        .await
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("initialize budget settings fence".into())
        })?;
    Ok(())
}

async fn policy(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
    session: &mut ClientSession,
) -> Result<Option<BudgetPolicy>, ApplyError> {
    // A real write also fences activation against previously disabled admissions.
    // The additive fence is outside the strict settings record.
    let result = store
        .budget_settings
        .update_one(
            doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            doc! {"$inc": {"model_budget_admission_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(domain(BudgetError::Unavailable));
    }
    let document = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let settings: layer2_core::PlatformSettings = mongodb::bson::deserialize_from_document(
        document
            .get_document("settings")
            .map_err(|_| domain(BudgetError::Unavailable))?
            .clone(),
    )
    .map_err(|_| domain(BudgetError::Unavailable))?;
    settings.model_budget.validate().map_err(domain)?;
    let Some(policy) = settings.model_budget.for_owner(owner) else {
        return Ok(None);
    };
    Ok(Some(policy.clone()))
}

async fn owner_budget(
    store: &MongoSandboxStore,
    agent: &Agent,
    policy: &BudgetPolicy,
    now: u64,
    session: &mut ClientSession,
) -> Result<OwnerBudget, ApplyError> {
    match store
        .model_budget
        .find_one(doc! {"_id": owner_key(agent)})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    {
        Some(document) => decode(&document),
        None => baseline::recorded_owner_usage(store, &agent.owner, policy, now, session).await,
    }
}

fn owner_key(agent: &Agent) -> String {
    format!("owner:{}", agent.owner.as_str())
}
fn turn_key(agent: &Agent, turn: u64) -> String {
    format!("turn:{}:{turn}", agent.id.as_str())
}
fn command_key(command: &str) -> String {
    format!("command:{command}")
}

/// Reserve at the durable command boundary, after authorization and exact replay.
pub(super) async fn reserve_command(
    store: &MongoSandboxStore,
    agent: &Agent,
    command: &str,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(policy) = policy(store, &agent.owner, session).await? else {
        return Ok(());
    };
    reserve(store, agent, command_key(command), policy, now, session).await
}

async fn reserve(
    store: &MongoSandboxStore,
    agent: &Agent,
    key: String,
    policy: BudgetPolicy,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    require_runtime(agent)?;
    if let Some(document) = store
        .model_budget
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    {
        let existing: TurnRecord = decode(&document)?;
        if existing.owner != agent.owner || existing.agent_id != agent.id {
            return Err(domain(BudgetError::Unavailable));
        }
        return Ok(());
    }
    let mut owner = owner_budget(store, agent, &policy, now, session).await?;
    owner.reserve(&policy, now).map_err(domain)?;
    save(store, &owner_key(agent), &owner, session).await?;
    save(
        store,
        &key,
        &TurnRecord {
            owner: agent.owner.clone(),
            agent_id: agent.id.clone(),
            turn_sequence: None,
            runtime_generation: None,
            reservation: TurnReservation::new(policy, now),
        },
        session,
    )
    .await
}

/// This participates in the Agent/log transaction for every turn entry path.
pub(super) async fn apply(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentConversationMutation,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let now = layer2_core::seconds_from_rfc3339(&mutation.appended_at)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    for log in &mutation.agent_logs {
        if let AgentLogKind::TurnStarted { turn_sequence, .. } = log {
            start_turn(store, current, mutation, *turn_sequence, now, session).await?;
        }
    }
    let Some(turn) = current.conversation.active_turn else {
        return Ok(());
    };
    let key = turn_key(current, turn);
    let Some(document) = store
        .model_budget
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(());
    };
    let mut record: TurnRecord = decode(&document)?;
    if record.reservation.settled {
        return Ok(());
    }
    if record.owner != current.owner || record.turn_sequence != Some(turn) {
        return Err(domain(BudgetError::Unavailable));
    }
    if let Some(usage) = mutation
        .agent
        .conversation
        .observations
        .usage
        .as_ref()
        .and_then(|usage| usage.current_turn.as_ref())
        .filter(|usage| usage.turn_sequence == turn)
    {
        // Only a new usage observation refreshes freshness, never a text/tool frame.
        if mutation
            .agent_logs
            .iter()
            .any(|log| matches!(log, AgentLogKind::UsageUpdated { .. }))
        {
            if let Err(reason) =
                record
                    .reservation
                    .observe(usage.input_tokens, usage.output_tokens, now)
            {
                record.reservation.stop.get_or_insert(reason);
            }
        }
    }
    if mutation.agent.conversation.active_turn != Some(turn)
        || record
            .reservation
            .observed
            .saturating_add(record.reservation.policy.max_unobserved_units)
            > record.reservation.reserved_units
    {
        let mut owner: OwnerBudget = store
            .model_budget
            .find_one(doc! {"_id": owner_key(current)})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .as_ref()
            .map(decode)
            .transpose()?
            .ok_or_else(|| domain(BudgetError::Unavailable))?;
        // Existing observations do not prove a complete provider settlement.
        // Keep the full allowance as a debit until an authoritative source can reconcile.
        owner
            .retain_observed_liability(&mut record.reservation)
            .map_err(domain)?;
        if mutation.agent.conversation.active_turn != Some(turn) {
            owner.settle(&record.reservation, now).map_err(domain)?;
            record.reservation.settled = true;
        }
        save(store, &owner_key(current), &owner, session).await?;
    }
    save(store, &key, &record, session).await
}

async fn start_turn(
    store: &MongoSandboxStore,
    agent: &Agent,
    mutation: &AgentConversationMutation,
    turn: u64,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let key = turn_key(agent, turn);
    let live_policy = policy(store, &agent.owner, session).await?;
    let command = mutation
        .command_claim
        .as_ref()
        .map(|claim| command_key(claim.command_id.as_str()));
    let prior = match &command {
        Some(command) => store
            .model_budget
            .find_one(doc! {"_id": command})
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?,
        None => None,
    };
    let mut record: TurnRecord = match prior.as_ref().map(decode).transpose()? {
        Some(record) => record,
        None => {
            let Some(policy) = live_policy.clone() else {
                return Ok(());
            };
            reserve(store, agent, key.clone(), policy, now, session).await?;
            let document = store
                .model_budget
                .find_one(doc! {"_id": &key})
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?
                .ok_or_else(|| domain(BudgetError::Unavailable))?;
            decode(&document)?
        }
    };
    if record.owner != agent.owner
        || record.agent_id != agent.id
        || record.reservation.settled
        || record
            .turn_sequence
            .is_some_and(|existing| existing != turn)
    {
        return Err(domain(BudgetError::Unavailable));
    }
    require_runtime(agent)?;
    if let Some(policy) = &live_policy {
        record.reservation.revalidate(policy).map_err(domain)?;
        let mut owner = owner_budget(store, agent, policy, now, session).await?;
        owner.advance(now).map_err(domain)?;
        if owner
            .spent
            .checked_add(owner.reserved)
            .is_none_or(|total| total > policy.owner_day_limit)
        {
            return Err(domain(BudgetError::OwnerExhausted));
        }
    }
    record.turn_sequence = Some(turn);
    record.runtime_generation = Some(mutation.runtime_generation);
    record.reservation.last_usage_at = now;
    save(store, &key, &record, session).await?;
    if let Some(command) = command.filter(|_| prior.is_some()) {
        // Retain the immutable command binding; its reservation transfers, not duplicates.
        save(store, &command, &record, session).await?;
    }
    Ok(())
}

fn require_runtime(agent: &Agent) -> Result<(), ApplyError> {
    agent
        .capabilities
        .require([
            layer2_core::RuntimeCapability::Usage,
            layer2_core::RuntimeCapability::Interrupt,
        ])
        .map_err(|_| domain(BudgetError::Unavailable))
}

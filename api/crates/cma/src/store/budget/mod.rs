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
mod certification;
#[cfg(test)]
mod certification_tests;
mod counter_accounting;
#[cfg(test)]
mod counter_tests;
mod counters;
pub(crate) mod inventory;
mod period;
mod period_storage;
mod requests;
mod source;
mod spend_ledger;
pub(super) use spend_ledger::RegistrationReader;
mod stop_authority;
mod transitions;
mod turns;
pub(super) use cancellation::release_unexecuted;
pub(super) use period::require_open_protected;
pub(super) use period::{now_seconds, require_open};
#[cfg(test)]
pub(super) use period::{period_key, require_open_with};
pub(super) use source::PlatformStopCache;
pub(super) use stop_authority::fence_stop;
pub(super) use turns::{settle_owner_stop, settle_runtime_loss};

pub(super) const COLLECTION: &str = "model_budget_reservations";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRecord {
    owner: layer2_core::SubjectId,
    agent_id: layer2_core::AgentId,
    turn_sequence: Option<u64>,
    runtime_generation: Option<u64>,
    reservation: TurnReservation,
    /// The platform period this liability was charged to; absent for
    /// reservations made before a platform policy was enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    period_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    period_shard: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    certification: Option<certification::Certification>,
}

fn domain(reason: BudgetError) -> ApplyError {
    ApplyError::Domain(layer2_core::sandbox::store::SandboxStoreError::Budget(
        reason,
    ))
}

fn decode<T: serde::de::DeserializeOwned>(document: &Document) -> Result<T, ApplyError> {
    if document
        .get_document("record")
        .is_ok_and(certification::present)
    {
        return Err(domain(BudgetError::Unavailable));
    }
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
    if record.as_document().is_some_and(certification::present) {
        return Err(domain(BudgetError::Unavailable));
    }
    // The target may be a canonical turn copied from a command reservation.
    store
        .model_budget
        .replace_one(
            doc! {"_id": key, "record.certification": {"$exists": false}},
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
    initial_settings: super::settings_defaults::InitialSettings,
) -> Result<(), layer2_core::StoreError> {
    let settings = mongodb::bson::serialize_to_bson(&initial_settings.platform())
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
pub(super) fn turn_key(agent: &Agent, turn: u64) -> String {
    format!("turn:{}:{turn}", agent.id.as_str())
}

pub(super) fn validate_stale_turn_reservation(
    row: &Document,
    agent: &Agent,
    turn: u64,
    settled: bool,
) -> Result<(), ApplyError> {
    let record: TurnRecord = decode(row)?;
    if record.agent_id != agent.id
        || record.owner != agent.owner
        || record.turn_sequence != Some(turn)
        || record.runtime_generation != agent.conversation.runtime_generation
    {
        return Err(domain(BudgetError::Unavailable));
    }
    if record.reservation.settled != settled || (!settled && record.reservation.stop.is_some()) {
        return Err(ApplyError::Domain(
            layer2_core::sandbox::store::SandboxStoreError::Conflict,
        ));
    }
    Ok(())
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
    cause: layer2_core::admission::AdmissionCause,
    existing_consumer: bool,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let snapshot = period::fenced_snapshot(store, &agent.owner, session).await?;
    let Some(policy) = snapshot.owner.clone() else {
        period::require_unbudgeted(store, &snapshot, now, cause, session).await?;
        return require_command_capacity(
            store,
            agent,
            &snapshot,
            cause,
            existing_consumer,
            session,
        )
        .await;
    };
    reserve(
        store,
        agent,
        command_key(command),
        policy,
        &snapshot,
        now,
        session,
    )
    .await?;
    period::require_operator_open(&snapshot, cause)?;
    require_command_capacity(store, agent, &snapshot, cause, existing_consumer, session).await
}

async fn require_command_capacity(
    store: &MongoSandboxStore,
    agent: &Agent,
    snapshot: &period::PolicySnapshot,
    cause: layer2_core::admission::AdmissionCause,
    existing_consumer: bool,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if cause == layer2_core::admission::AdmissionCause::Continuation {
        return Ok(());
    }
    super::admission_capacity::require_sessions(
        store,
        agent,
        &snapshot.ceilings,
        cause,
        existing_consumer,
        session,
    )
    .await
}

async fn reserve(
    store: &MongoSandboxStore,
    agent: &Agent,
    key: String,
    policy: BudgetPolicy,
    snapshot: &period::PolicySnapshot,
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
    let charge = period::admit_liability(
        store,
        snapshot,
        &agent.owner,
        policy.turn_limit,
        now,
        session,
    )
    .await?;
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
            period_start: charge.map(|charge| charge.start),
            period_shard: charge.and_then(|charge| charge.shard),
            certification: None,
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
    turns::apply_conversation(store, current, mutation, now, session).await
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
    // Publication has already validated this server-attributed execution fence
    // against the current binding and parent in this same transaction.
    let existing_consumer = mutation
        .execution_fence
        .as_ref()
        .is_some_and(|fence| fence.binding_authority().is_some());
    let snapshot = period::fenced_snapshot(store, &agent.owner, session).await?;
    let live_policy = snapshot.owner.clone();
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
                // Activation after command admission still requires a valued
                // reservation before the first upstream turn is dispatched.
                period::require_unbudgeted(
                    store,
                    &snapshot,
                    now,
                    layer2_core::admission::AdmissionCause::Continuation,
                    session,
                )
                .await?;
                return super::admission_capacity::require_sessions(
                    store,
                    agent,
                    &snapshot.ceilings,
                    layer2_core::admission::AdmissionCause::Continuation,
                    existing_consumer,
                    session,
                )
                .await;
            };
            reserve(store, agent, key.clone(), policy, &snapshot, now, session).await?;
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
    if prior.is_some() {
        // The admitted hold remains valued if its owner policy is removed;
        // current platform STOP authority still applies before dispatch.
        period::require_open(
            store,
            now,
            layer2_core::admission::AdmissionCause::Continuation,
            session,
        )
        .await?;
    }
    if let Some(policy) = &live_policy {
        record.reservation.revalidate(policy).map_err(domain)?;
        let mut owner = owner_budget(store, agent, policy, now, session).await?;
        owner.advance(policy, now).map_err(domain)?;
        if owner
            .spent
            .checked_add(owner.reserved)
            .is_none_or(|total| total > policy.owner_day_limit)
        {
            return Err(domain(BudgetError::OwnerExhausted));
        }
    }
    super::admission_capacity::require_sessions(
        store,
        agent,
        &snapshot.ceilings,
        layer2_core::admission::AdmissionCause::Continuation,
        existing_consumer,
        session,
    )
    .await?;
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

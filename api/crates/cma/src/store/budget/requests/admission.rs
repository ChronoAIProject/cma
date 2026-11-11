use super::*;
use crate::store::agents::read_agent;
use layer2_core::sandbox::AgentStatus;

pub(super) async fn reserve(
    store: &MongoSandboxStore,
    scope: &RequestScope,
    request: &ModelRequest,
    session: &mut ClientSession,
) -> Result<Option<RequestHold>, ApplyError> {
    let key = request_key(scope);
    if store
        .model_budget
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some()
    {
        // An existing hold is durable evidence of an already-issued permit.
        return Err(domain(BudgetError::Unavailable));
    }
    let document = store
        .agents
        .find_one(doc! {"_id": scope.agent_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let agent = read_agent(&document).map_err(ApplyError::Domain)?;
    if agent.status == AgentStatus::Deleted || scope.turn_sequence > agent.conversation.last_seq {
        return Err(domain(BudgetError::Unavailable));
    }
    let snapshot = super::super::period::fenced_snapshot(store, &agent.owner, session).await?;
    let Some(policy) = snapshot.owner.clone() else {
        return Ok(None);
    };
    let settings = store
        .budget_settings
        .find_one(doc! {"_id": "platform"})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let settings: layer2_core::PlatformSettings = mongodb::bson::deserialize_from_document(
        settings
            .get_document("settings")
            .map_err(|_| domain(BudgetError::Unavailable))?
            .clone(),
    )
    .map_err(|_| domain(BudgetError::Unavailable))?;
    let model = settings
        .model_budget
        .request_models
        .iter()
        .find(|model| model.model == request.model)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let liability = model.quote(&policy, request).map_err(domain)?;
    let parent = store
        .model_budget
        .find_one(doc! {"_id": super::super::turn_key(&agent, scope.turn_sequence)})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let parent: TurnRecord = decode(&parent)?;
    if parent.owner != agent.owner
        || parent.agent_id != agent.id
        || parent.turn_sequence != Some(scope.turn_sequence)
        || !parent.reservation.settled
        || parent.reservation.policy.input_token_units != policy.input_token_units
        || parent.reservation.policy.output_token_units != policy.output_token_units
        || parent.reservation.policy.day_timezone != policy.day_timezone
    {
        return Err(domain(BudgetError::Unavailable));
    }
    let ledger_key = ledger_key(scope);
    let mut ledger: RequestTurnLedger = store
        .model_budget
        .find_one(doc! {"_id": &ledger_key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(decode)
        .transpose()?
        .unwrap_or_default();
    let limit = policy.turn_limit.min(parent.reservation.policy.turn_limit);
    if ledger.bound_violated {
        return Err(domain(BudgetError::Unavailable));
    }
    // A terminal event is not final settlement. Preserve the complete parent
    // allowance until the authoritative source proves what was actually spent.
    let parent_spent = if parent.reservation.final_usage_known {
        parent.reservation.observed
    } else {
        parent
            .reservation
            .observed
            .max(parent.reservation.reserved_units)
    };
    let next_reserved = ledger
        .reserved
        .checked_add(liability)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    if parent_spent
        .checked_add(ledger.spent)
        .and_then(|spent| spent.checked_add(next_reserved))
        .is_none_or(|total| total > limit)
    {
        return Err(domain(BudgetError::TurnExhausted));
    }
    let parent_calls = parent
        .reservation
        .final_model_calls
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let next_calls = ledger
        .calls
        .checked_add(1)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let call_limit = policy
        .max_calls_per_turn
        .min(parent.reservation.policy.max_calls_per_turn);
    if parent_calls
        .checked_add(next_calls)
        .is_none_or(|calls| calls > call_limit)
    {
        return Err(domain(BudgetError::TurnExhausted));
    }
    let timestamp = now(store).map_err(domain)?;
    let mut owner = super::super::owner_budget(store, &agent, &policy, timestamp, session).await?;
    owner
        .reserve_liability(&policy, liability, timestamp)
        .map_err(domain)?;
    let period_start =
        super::super::period::admit_liability(store, &snapshot, liability, timestamp, session)
            .await?;
    ledger.reserved = next_reserved;
    // Every granted permit counts, including uncertain dispatch and settlement.
    ledger.calls = next_calls;
    let hold = RequestHold {
        scope: scope.clone(),
        request: request.clone(),
        input_tokens_bound: request.input_bytes + model.input_overhead_tokens,
    };
    let record = RequestRecord {
        hold: hold.clone(),
        owner: agent.owner.clone(),
        policy,
        reserved: liability,
        settled: None,
        period_start,
    };
    save(store, &super::super::owner_key(&agent), &owner, session).await?;
    save(store, &ledger_key, &ledger, session).await?;
    save(store, &key, &record, session).await?;
    Ok(Some(hold))
}

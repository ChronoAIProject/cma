use super::*;

pub(super) async fn settle(
    store: &MongoSandboxStore,
    hold: &RequestHold,
    usage: RequestUsage,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let key = request_key(&hold.scope);
    let raw = store
        .model_budget
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let mut record: RequestRecord = decode(&raw)?;
    if record.hold != *hold {
        return Err(domain(BudgetError::Unavailable));
    }
    if let Some(previous) = record.settled {
        return if previous == usage {
            Ok(())
        } else {
            Err(domain(BudgetError::Unavailable))
        };
    }
    let actual = record
        .policy
        .value_usage(usage.input_tokens, usage.output_tokens)
        .map_err(domain)?;
    let owner_key = format!("owner:{}", record.owner.as_str());
    let raw = store
        .model_budget
        .find_one(doc! {"_id": &owner_key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let mut owner: OwnerBudget = decode(&raw)?;
    owner
        .advance(&record.policy, now(store).map_err(domain)?)
        .map_err(domain)?;
    if owner.token_units
        != Some([
            record.policy.input_token_units,
            record.policy.output_token_units,
        ])
    {
        return Err(domain(BudgetError::Unavailable));
    }
    let ledger_key = ledger_key(&hold.scope);
    let raw = store
        .model_budget
        .find_one(doc! {"_id": &ledger_key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let mut ledger: RequestTurnLedger = decode(&raw)?;
    ledger.bound_violated |= usage.input_tokens > hold.input_tokens_bound
        || usage.output_tokens > hold.request.max_output_tokens;
    owner.reserved = owner
        .reserved
        .checked_sub(record.reserved)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    ledger.reserved = ledger
        .reserved
        .checked_sub(record.reserved)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    owner.spent = owner
        .spent
        .checked_add(actual)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    ledger.spent = ledger
        .spent
        .checked_add(actual)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let reserved = record.reserved;
    super::super::period::mirror(store, record.period_start, session, |platform| {
        platform.settle(reserved, actual)
    })
    .await?;
    record.settled = Some(usage);
    save(store, &owner_key, &owner, session).await?;
    save(store, &ledger_key, &ledger, session).await?;
    save(store, &key, &record, session).await
}

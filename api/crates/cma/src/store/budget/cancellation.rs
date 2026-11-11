use layer2_core::budget::{BudgetError, OwnerBudget};
use layer2_core::sandbox::agent_io::AdmittedAgentCommand;
use mongodb::{bson::doc, ClientSession};

use super::{command_key, decode, domain, save, ApplyError, MongoSandboxStore, TurnRecord};

pub(in crate::store) async fn release_unexecuted(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if !command.status.is_terminal() || command.work.dispatch.is_some() {
        return Ok(());
    }
    let key = command_key(command.receipt.envelope.command_id.as_str());
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
    // Only durable proof of no dispatch releases an unused reservation.
    if record.turn_sequence.is_some() || record.agent_id != command.receipt.envelope.agent_id {
        return Err(domain(BudgetError::Unavailable));
    }
    let owner_key = format!("owner:{}", record.owner.as_str());
    let mut owner: OwnerBudget = store
        .model_budget
        .find_one(doc! {"_id": &owner_key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(decode)
        .transpose()?
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    let units = record.reservation.reserved_units;
    owner.reserved = owner
        .reserved
        .checked_sub(units)
        .ok_or_else(|| domain(BudgetError::Unavailable))?;
    super::period::mirror(store, record.period_start, session, |ledger| {
        ledger.release(units)
    })
    .await?;
    record.reservation.settled = true;
    save(store, &owner_key, &owner, session).await?;
    save(store, &key, &record, session).await
}

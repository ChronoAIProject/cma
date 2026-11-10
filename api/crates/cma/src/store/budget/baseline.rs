use layer2_core::budget::{BudgetError, BudgetPolicy, OwnerBudget};
use mongodb::{bson::doc, ClientSession};

use super::{domain, ApplyError, MongoSandboxStore};

pub(super) async fn recorded_owner_usage(
    store: &MongoSandboxStore,
    owner: &layer2_core::SubjectId,
    policy: &BudgetPolicy,
    now: u64,
    session: &mut ClientSession,
) -> Result<OwnerBudget, ApplyError> {
    // Bootstrap conservatively from all retained cumulative model usage, rather
    // than treating an uninitialized accounting row as an unspent owner.
    let mut cursor = store
        .agents
        .find(doc! {"owner": owner.as_str()})
        .limit(1025)
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let mut result = OwnerBudget {
        token_units: Some([policy.input_token_units, policy.output_token_units]),
        ..OwnerBudget::default()
    };
    result.advance(now).map_err(domain)?;
    let mut count = 0;
    while cursor
        .advance(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    {
        count += 1;
        if count > 1024 {
            return Err(domain(BudgetError::Unavailable));
        }
        let document = cursor.deserialize_current().map_err(ApplyError::Driver)?;
        let agent = super::super::agents::read_agent(&document).map_err(ApplyError::Domain)?;
        if let Some(usage) = &agent.conversation.observations.usage {
            let value = policy
                .value_usage(usage.input_tokens, usage.output_tokens)
                .map_err(domain)?;
            result.spent = result
                .spent
                .checked_add(value)
                .filter(|value| *value <= i64::MAX as u64)
                .ok_or_else(|| domain(BudgetError::Unavailable))?;
        }
    }
    Ok(result)
}

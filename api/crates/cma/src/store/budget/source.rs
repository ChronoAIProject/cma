use layer2_core::budget::{BudgetError, BudgetPolicy, BudgetStop, OwnerBudget, SpendSource};
use layer2_core::sandbox::{store::AgentStore, Agent};
use layer2_core::AgentId;
use mongodb::bson::doc;

use super::{decode, owner_key, turn_key, MongoSandboxStore, TurnRecord, DOCUMENT_SCHEMA_VERSION};

#[async_trait::async_trait]
impl SpendSource for MongoSandboxStore {
    async fn stop_requested(
        &self,
        agent_id: &AgentId,
        runtime_generation: u64,
        now_seconds: u64,
    ) -> Result<Option<BudgetStop>, BudgetError> {
        let agent = self
            .get_agent(agent_id)
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .ok_or(BudgetError::Unavailable)?;
        if agent.conversation.runtime_generation != Some(runtime_generation) {
            return Ok(None);
        }
        let Some(turn) = agent.conversation.active_turn else {
            return Ok(None);
        };
        let stop_key = format!("stop:{}:{turn}", agent.id.as_str());
        let prior = self
            .model_budget
            .find_one(doc! {"_id": &stop_key})
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        let reason = match prior {
            Some(document) => {
                Some(decode::<BudgetError>(&document).map_err(|_| BudgetError::Unavailable)?)
            }
            None => self.current_stop(&agent, turn, now_seconds).await?,
        };
        let Some(reason) = reason else {
            return Ok(None);
        };
        // Persist independently of the reservation. Activation can discover a
        // running turn with no reservation; disabling the policy must not erase
        // a stop already requested for that turn. First writer chooses the reason.
        self.model_budget.update_one(doc! {"_id": &stop_key}, doc! {"$setOnInsert": {
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "record": mongodb::bson::serialize_to_bson(&reason).map_err(|_| BudgetError::Unavailable)?
        }}).upsert(true).await.map_err(|_| BudgetError::Unavailable)?;
        let document = self
            .model_budget
            .find_one(doc! {"_id": &stop_key})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .ok_or(BudgetError::Unavailable)?;
        let reason = decode(&document).map_err(|_| BudgetError::Unavailable)?;
        Ok(Some(BudgetStop {
            agent_id: agent.id,
            runtime_generation,
            turn_sequence: turn,
            reason,
        }))
    }
}

impl MongoSandboxStore {
    async fn current_stop(
        &self,
        agent: &Agent,
        turn: u64,
        now: u64,
    ) -> Result<Option<BudgetError>, BudgetError> {
        let policy = self.current_budget_policy(agent).await?;
        let document = self
            .model_budget
            .find_one(doc! {"_id": turn_key(agent, turn)})
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        let Some(document) = document else {
            return Ok(policy.map(|_| BudgetError::Unavailable));
        };
        let mut record: TurnRecord = decode(&document).map_err(|_| BudgetError::Unavailable)?;
        if record.owner != agent.owner
            || record.agent_id != agent.id
            || record.turn_sequence != Some(turn)
            || record.reservation.settled
        {
            return Ok(Some(BudgetError::Unavailable));
        }
        if let Some(reason) = record.reservation.stop_reason(now) {
            return Ok(Some(reason));
        }
        if let Some(policy) = &policy {
            if let Err(reason) = record.reservation.revalidate(policy) {
                return Ok(Some(reason));
            }
            let document = self
                .model_budget
                .find_one(doc! {"_id": owner_key(agent)})
                .await
                .map_err(|_| BudgetError::Unavailable)?
                .ok_or(BudgetError::Unavailable)?;
            let mut owner: OwnerBudget = decode(&document).map_err(|_| BudgetError::Unavailable)?;
            owner.advance(now)?;
            if owner
                .spent
                .checked_add(owner.reserved)
                .is_none_or(|total| total > policy.owner_day_limit)
            {
                return Ok(Some(BudgetError::OwnerExhausted));
            }
        }
        Ok(None)
    }

    async fn current_budget_policy(
        &self,
        agent: &Agent,
    ) -> Result<Option<BudgetPolicy>, BudgetError> {
        let Some(document) = self
            .budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .map_err(|_| BudgetError::Unavailable)?
        else {
            return Ok(None);
        };
        let settings: layer2_core::PlatformSettings = mongodb::bson::deserialize_from_document(
            document
                .get_document("settings")
                .map_err(|_| BudgetError::Unavailable)?
                .clone(),
        )
        .map_err(|_| BudgetError::Unavailable)?;
        settings.model_budget.validate()?;
        Ok(settings.model_budget.for_owner(&agent.owner).cloned())
    }
}

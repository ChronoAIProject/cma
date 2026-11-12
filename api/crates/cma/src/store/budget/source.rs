use std::time::{Duration, Instant};

use layer2_core::budget::{
    BudgetError, BudgetStop, ClosureReason, OwnerBudget, PlatformBudgetPolicy,
    PlatformPeriodLedger, SpendSource,
};
use layer2_core::sandbox::{store::AgentStore, Agent};
use layer2_core::AgentId;
use mongodb::bson::doc;

use super::period::{period_key, snapshot, PolicySnapshot};
use super::{decode, owner_key, turn_key, MongoSandboxStore, TurnRecord, DOCUMENT_SCHEMA_VERSION};

/// How long one plain read of the period row answers the stop poll.
const PLATFORM_STOP_TTL: Duration = Duration::from_secs(5);

/// Process-local hints from platform reads (concurrency rules 3, 5): whether
/// the last settings read showed an enabled platform policy, which schedules
/// period-row writers, and the last stop read of the period row. A stale entry
/// can delay a stop by one TTL; it is never authority to admit anything.
#[derive(Default)]
pub(in crate::store) struct PlatformStopCache {
    platform_enabled: Option<bool>,
    stop: Option<StopRead>,
}

struct StopRead {
    period_key: String,
    closed: bool,
    read_at: Instant,
}

impl MongoSandboxStore {
    /// Whether period-row writers should be scheduled; unknown counts as enabled.
    pub(in crate::store) fn platform_enabled_hint(&self) -> bool {
        self.platform
            .stop_cache
            .lock()
            .map(|cache| cache.platform_enabled.unwrap_or(true))
            .unwrap_or(true)
    }

    pub(super) fn note_platform_enabled(&self, enabled: bool) {
        if let Ok(mut cache) = self.platform.stop_cache.lock() {
            cache.platform_enabled = Some(enabled);
        }
    }

    /// `SpendStop` on the current period row, read without a session or the
    /// settings fence and served from the cache within its TTL.
    async fn platform_stopped(
        &self,
        platform: Option<&PlatformBudgetPolicy>,
    ) -> Result<bool, BudgetError> {
        self.note_platform_enabled(platform.is_some());
        let Some(platform) = platform else {
            return Ok(false);
        };
        let key = period_key(platform.period_bounds()?.0);
        if let Some(closed) = self.cached_stop(&key) {
            return Ok(closed);
        }
        let closed = match self
            .model_budget
            .find_one(doc! {"_id": &key})
            .await
            .map_err(|_| BudgetError::Unavailable)?
        {
            Some(document) => decode::<PlatformPeriodLedger>(&document)
                .map_err(|_| BudgetError::Unavailable)?
                .closures
                .iter()
                .any(|closure| closure.reason == ClosureReason::SpendStop),
            None => false,
        };
        if let Ok(mut cache) = self.platform.stop_cache.lock() {
            cache.stop = Some(StopRead {
                period_key: key,
                closed,
                read_at: Instant::now(),
            });
        }
        Ok(closed)
    }

    fn cached_stop(&self, key: &str) -> Option<bool> {
        let cache = self.platform.stop_cache.lock().ok()?;
        cache
            .stop
            .as_ref()
            .filter(|read| read.period_key == key && read.read_at.elapsed() < PLATFORM_STOP_TTL)
            .map(|read| read.closed)
    }

    #[cfg(test)]
    pub(in crate::store) fn age_platform_stop_cache(&self, by: Duration) {
        if let Ok(mut cache) = self.platform.stop_cache.lock() {
            if let Some(read) = cache.stop.as_mut() {
                read.read_at = read.read_at.checked_sub(by).unwrap_or(read.read_at);
            }
        }
    }
}

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
        let snapshot = self.current_budget_snapshot(agent).await?;
        // The platform stop binds every active turn, budgeted owner or not.
        if self
            .platform_stopped(snapshot.as_ref().and_then(|value| value.platform.as_ref()))
            .await?
        {
            return Ok(Some(BudgetError::PlatformExhausted));
        }
        let policy = snapshot.and_then(|value| value.owner);
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
            if let Some(reason) = record.reservation.stop_reason(now) {
                return Ok(Some(reason));
            }
            let document = self
                .model_budget
                .find_one(doc! {"_id": owner_key(agent)})
                .await
                .map_err(|_| BudgetError::Unavailable)?
                .ok_or(BudgetError::Unavailable)?;
            let mut owner: OwnerBudget = decode(&document).map_err(|_| BudgetError::Unavailable)?;
            owner.advance(policy, now)?;
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

    async fn current_budget_snapshot(
        &self,
        agent: &Agent,
    ) -> Result<Option<PolicySnapshot>, BudgetError> {
        let Some(document) = self
            .budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .map_err(|_| BudgetError::Unavailable)?
        else {
            return Ok(None);
        };
        snapshot(&document, Some(&agent.owner)).map(Some)
    }
}

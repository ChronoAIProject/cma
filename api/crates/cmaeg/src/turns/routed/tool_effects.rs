//! Delayed replies have distinct FIFO work IDs and retain original capability
//! authority. A completed parent target is never reopened for a later effect.
mod executor;
mod gate;
mod model;
#[cfg(test)]
mod mongo_answer_receipt_tests;
#[cfg(test)]
mod mongo_arbitration_tests;
#[cfg(test)]
mod mongo_failure_tests;
#[cfg(test)]
mod mongo_history_tests;
#[cfg(test)]
mod mongo_issue_2873;
#[cfg(test)]
mod mongo_lane_tests;
#[cfg(test)]
mod mongo_operator_expiry_tests;
#[cfg(test)]
mod mongo_operator_http_tests;
#[cfg(test)]
mod mongo_operator_tests;
#[cfg(test)]
mod mongo_progress_fixture;
#[cfg(test)]
mod mongo_progress_tests;
#[cfg(test)]
mod mongo_renewal_tests;
#[cfg(test)]
mod mongo_replay_progress_tests;
#[cfg(test)]
mod mongo_resume_tests;
#[cfg(test)]
mod mongo_rollback_tests;
#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
pub(super) mod mongo_webhook_renewal_tests;
mod store;
use super::{
    dispatcher::{admission_error, current_target},
    model::delivery_event,
    tool_replies::same_event,
    AcceptedAnswerIdentity, ExecutionError, Phase, RoutedToolReplies, TargetState,
};
use crate::{
    auth::AgentCaller,
    delivery::intents::IntentReceipt,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::effects::EffectExecutor,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{ReplyTargetPayload, ReplyTargetRef},
};
use model::Payload;
use std::{sync::Arc, time::Duration};
use store::Store;

impl RoutedToolReplies {
    pub fn replica_effect_executor(self: &Arc<Self>) -> Arc<dyn EffectExecutor> {
        Arc::new(executor::Handler(self.clone()))
    }
    /// None retains the pre-replica path. Enabled mode never falls through to an
    /// unfenced send; a queued operation returns Busy until its receipt exists.
    pub async fn reply_delayed(
        &self,
        caller: AgentCaller,
        capability: &ReplyTargetPayload,
        key: &str,
        message: OutboundMessage,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        self.reply_delayed_inner(caller, capability, key, message, None)
            .await
    }

    pub(crate) async fn reply_delayed_answer(
        &self,
        caller: AgentCaller,
        capability: &ReplyTargetPayload,
        answer: AcceptedAnswerIdentity,
        message: OutboundMessage,
    ) -> Result<IntentReceipt, ExecutionError> {
        if !self.journal.accepted_answer_receipts {
            return Err(ExecutionError::Unsupported);
        }
        self.reply_delayed_inner(caller, capability, "", message, Some(answer))
            .await?
            .ok_or(ExecutionError::Unsupported)
    }

    async fn reply_delayed_inner(
        &self,
        caller: AgentCaller,
        capability: &ReplyTargetPayload,
        key: &str,
        message: OutboundMessage,
        answer: Option<AcceptedAnswerIdentity>,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        if self.journal.partitions.is_none() {
            return Ok(None);
        }
        if (answer.is_none()
            && (key.is_empty() || key.len() > 128 || !key.bytes().all(|b| (33..=126).contains(&b))))
            || message.structured_content.is_some()
        {
            return Err(ExecutionError::Invalid);
        }
        message.validate().map_err(|_| ExecutionError::Invalid)?;
        let now = (self.journal.clock)();
        ReplyTargetRef::seal(capability, &self.journal.keys)
            .map_err(|_| ExecutionError::Invalid)?
            .open(&self.journal.keys, now)
            .map_err(|error| {
                if matches!(error, cmaeg_core::seal::OpenError::Expired) {
                    ExecutionError::Expired
                } else {
                    ExecutionError::Invalid
                }
            })?;
        let binding = self
            .journal
            .tool_reply_binding(&capability.event)
            .await?
            .ok_or(ExecutionError::Unsupported)?;
        let plan = self
            .plans
            .load(&binding.identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == binding.target)
            .ok_or(ExecutionError::Corrupt)?;
        if plan.admission_id != binding.root
            || plan.request_hash != binding.request_hash
            || !same_event(&delivery_event(&plan, target), &capability.event, true)
        {
            return Err(ExecutionError::Corrupt);
        }
        self.authorize_delayed(caller, capability, &plan, target, answer.as_ref())
            .await?;
        if let Some(answer) = &answer {
            if let Some(receipt) = self
                .journal
                .admit_delayed_answer_payload(&plan, target, answer, &message)
                .await?
            {
                self.journal.require_reply_open(&plan, target).await?;
                return Ok(Some(receipt));
            }
        } else {
            let legacy = crate::tools::reply_targets::receipts::Store::new(
                self.journal.records.clone(),
                self.journal.keys.clone(),
            );
            if let Some(receipt) = legacy
                .lookup_target(capability, key, &message, &plan.identity.owner)
                .await
                .map_err(|error| match error.code {
                    cmaeg_core::problem::ProblemCode::Conflict => {
                        ExecutionError::IdempotencyConflict
                    }
                    _ => ExecutionError::Unavailable,
                })?
            {
                return Ok(Some(receipt));
            }
        }
        let payload = Payload {
            capability: capability.clone(),
            caller: caller.key_hash,
            message,
        };
        let store = Store(self.journal.clone());
        let root = match answer {
            Some(answer) => store.admit_answer(&plan, target, &payload, answer).await?,
            None => store.admit(&plan, target, key, &payload).await?,
        };
        self.journal.require_reply_open(&plan, target).await?;
        if let Some(receipt) = store.receipt(&root).await? {
            return Ok(Some(receipt));
        }
        self.journal
            .ensure_delayed_custody(&plan, target, &root.id)
            .await?;
        metrics::counter!("cmaeg_replica_delayed_reply_total", "outcome" => "queued").increment(1);
        // Discovery owns execution. This bounded wait only reads the same root;
        // cancellation of the HTTP caller cannot cancel accepted durable custody.
        let until = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let root = store
                .read(&root.id, &plan, target)
                .await?
                .ok_or(ExecutionError::Corrupt)?;
            if let Some(receipt) = store.receipt(&root).await? {
                return Ok(Some(receipt));
            }
            if tokio::time::Instant::now() >= until {
                return Err(ExecutionError::Busy);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn authorize_delayed(
        &self,
        caller: AgentCaller,
        capability: &ReplyTargetPayload,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        answer: Option<&AcceptedAnswerIdentity>,
    ) -> Result<Arc<crate::channels::resolver::ResolvedChannel>, ExecutionError> {
        self.journal.require_reply_open(plan, target).await?;
        let current = current_target(self.authority.as_ref(), plan, target).await?;
        let (execution, _) = self
            .journal
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Invalid)?;
        // A capability cannot manufacture a provider invocation or revive a
        // refused parent. Delayed authority only extends the reply window.
        if execution.root != plan.admission_id
            || execution.owner != plan.identity.owner
            || execution.request_hash != plan.request_hash
            || execution.body.is_none()
            || !matches!(
                execution.phase,
                Phase::Starting | Phase::Accepted { .. } | Phase::Completed { .. } | Phase::Unknown
            )
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        crate::tools::reply_targets::validate_current_target(
            caller,
            capability,
            &current,
            (self.journal.clock)(),
        )
        .map_err(|_| {
            if capability.expires_at <= (self.journal.clock)() {
                ExecutionError::Expired
            } else {
                ExecutionError::AuthorityChanged
            }
        })?;
        if !same_event(&delivery_event(plan, target), &capability.event, true) {
            return Err(ExecutionError::AuthorityChanged);
        }
        if let Some(answer) = answer {
            super::journal::validate(&execution, plan, target)?;
            if !matches!(&execution.phase,
                Phase::Accepted { response_id, .. } | Phase::Completed { response_id, .. }
                if response_id == &answer.response_id)
                || !matches!(
                    self.journal
                        .retained_body(&execution)
                        .await?
                        .delivery
                        .destination,
                    cmaeg_core::channel::ReplyChoice::Origin
                )
            {
                return Err(ExecutionError::AuthorityChanged);
            }
        }
        Ok(current)
    }
}

pub(crate) async fn operator_effect(
    journal: super::Journal,
    plan: &AdmittedPlan,
    target: &StoredTarget,
    id: &str,
) -> Result<(serde_json::Value, u64), ExecutionError> {
    let root = Store(journal)
        .read(id, plan, target)
        .await?
        .ok_or(ExecutionError::Corrupt)?;
    Ok((
        serde_json::to_value(&root).map_err(|_| ExecutionError::Corrupt)?,
        root.version,
    ))
}

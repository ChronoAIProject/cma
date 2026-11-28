//! Durable command driving. A runtime result never proves absence of an atomic
//! dispatch binding; only the authoritative ledger can decide the next action.

use super::*;
use crate::sandbox::store::{Page, PageRequest};
use crate::{ids::AgentCommandId, now_rfc3339, rfc3339_at};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[path = "worker_ownership.rs"]
mod ownership;

pub(super) const IO_BOUND: Duration = Duration::from_secs(15);
const ATTEMPT_BOUND: Duration = Duration::from_secs(45);
const LEASE_LIFETIME: Duration = Duration::from_secs(30);
const RENEW_INTERVAL: Duration = Duration::from_secs(10);
const RENEW_BOUND: Duration = Duration::from_secs(5);
pub(super) const UNCLAIMED_RECHECK_DELAY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAttempt {
    Finished,
    Retry,
    RetryAttachment,
    Reject(CommandFailureCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandReadiness {
    Ready,
    Retry,
    WaitingForActiveTurn,
    WaitingForAttachment,
    Reject(CommandFailureCode),
}

/// Preparation failures retain their retry/terminal distinction across adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommandPreparationError {
    #[error("revival authority is temporarily unavailable")]
    AuthorityUnavailable,
    #[error("command or lifecycle state changed; recheck current authority")]
    StateChanged,
    #[error("revival dependency is temporarily unavailable")]
    DependencyUnavailable,
    #[error("command preparation exceeded its IO bound")]
    TimedOut,
    #[error("command preparation refused: {0:?}")]
    Rejected(CommandFailureCode),
}

/// Production implements this with actual current credential authorization and
/// the existing context/claimed-operation services. No actor is synthesized from
/// the envelope owner. Preparation may request revival but may never send input.
#[async_trait::async_trait]
pub trait AgentCommandExecution: Send + Sync {
    /// Fresh authority and target checks, with no lifecycle or runtime effects.
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness;
    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError>;
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt;
}

#[derive(Clone)]
pub struct AgentCommandWorker {
    pub(super) store: Arc<dyn AgentCommandWorkerStore>,
    pub(super) dispatched_recheck_delay: Duration,
    execution: Arc<dyn AgentCommandExecution>,
    ownership: Option<Arc<crate::agent_ownership::OwnershipCoordinator>>,
    holder: String,
    pub(super) model_funding: Option<Arc<crate::sandbox::model_credit::AgentTurnFunding>>,
}

impl AgentCommandWorker {
    pub fn new(
        store: Arc<dyn AgentCommandWorkerStore>,
        execution: Arc<dyn AgentCommandExecution>,
        holder: String,
    ) -> Result<Arc<Self>, CommandLedgerError> {
        Self::with_recheck_delay(store, execution, holder, Duration::ZERO)
    }

    pub fn with_recheck_delay(
        store: Arc<dyn AgentCommandWorkerStore>,
        execution: Arc<dyn AgentCommandExecution>,
        holder: String,
        dispatched_recheck_delay: Duration,
    ) -> Result<Arc<Self>, CommandLedgerError> {
        CommandClaimRequest {
            agent_id: crate::AgentId("validation".into()),
            holder: holder.clone(),
            now: now_rfc3339(),
            expires_at: rfc3339_at(SystemTime::now() + LEASE_LIFETIME),
        }
        .validate()?;
        Ok(Arc::new(Self {
            store,
            execution,
            ownership: None,
            holder,
            dispatched_recheck_delay,
            model_funding: None,
        }))
    }

    pub fn with_model_funding(
        &self,
        funding: Option<Arc<crate::sandbox::model_credit::AgentTurnFunding>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            model_funding: funding,
            ..self.clone()
        })
    }

    pub async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AdmittedAgentCommand>, CommandLedgerError> {
        let result = bounded(self.store.command_work_page(page)).await?;
        // Defend the driver against oversized or nonadvancing adapter pages.
        let checked = finish_command_work_page(result.items.clone(), page)?;
        if result.items.len() > usize::from(page.limit)
            || result.next.as_ref().is_some_and(|next| {
                result
                    .items
                    .last()
                    .is_none_or(|last| next != last.receipt.envelope.command_id.as_str())
            })
            || (result.next.is_some() && result.items.len() != usize::from(page.limit))
            || checked.next.is_some()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        Ok(result)
    }

    pub async fn run_one(&self, id: &AgentCommandId) -> Result<(), CommandLedgerError> {
        let delay = self.run_scheduled(id).await?;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }

    /// Drive one attempt without occupying a scheduler slot during retry pacing.
    /// The returned cooldown carries no lease or saved authorization; a later
    /// attempt must reread and reauthorize the durable command.
    pub async fn run_scheduled(&self, id: &AgentCommandId) -> Result<Duration, CommandLedgerError> {
        let Some(candidate) = bounded(self.store.get_command(id)).await? else {
            // An unconfirmed admission may still be visible in funding discovery.
            return Ok(self
                .model_funding
                .as_ref()
                .map_or(Duration::ZERO, |_| UNCLAIMED_RECHECK_DELAY));
        };
        candidate.validate()?;
        if &candidate.receipt.envelope.command_id != id {
            return Err(CommandLedgerError::Corrupt);
        }
        let funding_delay = self.reconcile_funding(&candidate).await;
        if candidate.status.is_terminal() {
            return Ok(funding_delay);
        }
        if candidate.work.dispatch.is_some() {
            return self
                .reconcile_dispatched(id, &candidate)
                .await
                .map(|delay| delay.max(funding_delay));
        }
        let delay = candidate.continuation_retry_delay(&now_rfc3339());
        if !delay.is_zero() && !candidate.queue_expired(&now_rfc3339()) {
            return Ok(delay);
        }
        if !candidate.awaiting_claim(&now_rfc3339()) {
            return Ok(Duration::ZERO);
        }
        let checked_at = tokio::time::Instant::now();
        let mut readiness_timed_out = false;
        let readiness = if candidate.queue_expired(&now_rfc3339()) {
            CommandReadiness::Reject(CommandFailureCode::QueueTimeout)
        } else {
            tokio::time::timeout(IO_BOUND, self.execution.check_queued(&candidate))
                .await
                .unwrap_or_else(|_| {
                    readiness_timed_out = true;
                    CommandReadiness::Retry
                })
        };
        if readiness != CommandReadiness::Ready {
            tracing::warn!(
                event = "command_readiness",
                command_id = %id,
                agent_id = %candidate.receipt.envelope.agent_id,
                outcome = ?readiness,
                timed_out = readiness_timed_out,
                "Queued command is not ready"
            );
        }
        match readiness {
            CommandReadiness::Ready => {}
            CommandReadiness::WaitingForActiveTurn | CommandReadiness::WaitingForAttachment => {
                let delay = api_message_recheck_delay(
                    &candidate.receipt.envelope,
                    &candidate.receipt.admitted_at,
                    &candidate.queue_deadline,
                    &now_rfc3339(),
                );
                return Ok(unclaimed_cooldown(&candidate, delay));
            }
            CommandReadiness::Retry => {
                let delay = unclaimed_recheck_delay(&candidate);
                if is_channel_message(&candidate) {
                    tracing::info!(
                        event = "channel_operation",
                        operation = "command_readiness",
                        stage = "check_queued",
                        outcome = "retry",
                        timed_out = readiness_timed_out,
                        elapsed_ms = checked_at.elapsed().as_millis() as u64,
                        retry_delay_ms = delay.as_millis() as u64,
                        "Channel readiness will be checked again"
                    );
                }
                return Ok(unclaimed_cooldown(&candidate, delay));
            }
            CommandReadiness::Reject(code) => {
                return self.reject_without_dispatch(id, &candidate, code).await;
            }
        }
        let Some(scope) = self.scope(&candidate, true).await else {
            return Ok(unclaimed_cooldown(
                &candidate,
                unclaimed_recheck_delay(&candidate),
            ));
        };
        scope.run(self.claim_and_drive(id, candidate)).await
    }

    // Preparation, native execution and settlement share this attempt's
    // captured epoch. A successor only reconciles separately read durable evidence.
    async fn claim_and_drive(
        &self,
        id: &AgentCommandId,
        candidate: AdmittedAgentCommand,
    ) -> Result<Duration, CommandLedgerError> {
        let now = SystemTime::now();
        let selected = bounded(self.store.claim_observed_command(
            CommandClaimRequest {
                agent_id: candidate.receipt.envelope.agent_id.clone(),
                holder: self.holder.clone(),
                now: rfc3339_at(now),
                expires_at: rfc3339_at(now + LEASE_LIFETIME),
            },
            id,
            candidate.revision,
        ))
        .await?;
        let Some(command) = selected else {
            // No short lane lease is held while a paused context is revived.
            // Cancellation leaves any admitted lifecycle work durable.
            let current = bounded(self.store.get_command(id))
                .await?
                .ok_or(CommandLedgerError::NotFound)?;
            current.validate()?;
            if current != candidate
                || !current.awaiting_claim(&now_rfc3339())
                || current.queue_expired(&now_rfc3339())
            {
                return Ok(Duration::ZERO);
            }
            let preparation =
                tokio::time::timeout(IO_BOUND, self.execution.prepare_queued(&current))
                    .await
                    .unwrap_or(Err(CommandPreparationError::TimedOut));
            if let Err(error) = preparation {
                tracing::warn!(event = "command_preparation", command_id = %id,
                    agent_id = %current.receipt.envelope.agent_id,
                    boundary = "prepare_queued", error_kind = ?error,
                    "Queued command preparation failed");
                if let CommandPreparationError::Rejected(code) = error {
                    // The revision CAS refuses stale rejection after another
                    // worker claims, dispatches, edits or cancels this command.
                    return bounded(self.store.reject_unclaimed_command(
                        id,
                        current.revision,
                        code,
                        now_rfc3339(),
                    ))
                    .await
                    .map(|()| Duration::ZERO);
                }
            }
            // Channel preparation or an explicit interruption may have just
            // cancelled the active turn. Revisit its replacement promptly,
            // still reacquiring authority and the lane on the next attempt.
            let delay = if (matches!(current.receipt.envelope.actor, AgentActor::Channel { .. })
                && current.receipt.envelope.source.kind == AgentSourceKind::Channel)
                || current.queue_priority().0 == 0
            {
                Duration::from_millis(250)
            } else {
                api_message_recheck_delay(
                    &current.receipt.envelope,
                    &current.receipt.admitted_at,
                    &current.queue_deadline,
                    &now_rfc3339(),
                )
            };
            return Ok(unclaimed_cooldown(&current, delay));
        };
        command.validate()?;
        let claim = command
            .work
            .lease
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?
            .claim
            .clone();
        command.require_claim(&claim, &now_rfc3339())?;
        if claim.holder != self.holder
            || claim.agent_id != candidate.receipt.envelope.agent_id
            || command.receipt != candidate.receipt
            || command.key_sha256 != candidate.key_sha256
            || command.target_turn != candidate.target_turn
            || command.revision
                != candidate
                    .revision
                    .checked_add(1)
                    .ok_or(CommandLedgerError::Corrupt)?
            || command.work.dispatch.is_some()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        if command.queue_expired(&now_rfc3339()) {
            return self
                .settle(
                    &command,
                    &claim,
                    CommandAttempt::Reject(CommandFailureCode::QueueTimeout),
                )
                .await;
        }
        let attempt = self.drive(&command, &claim).await;
        self.settle(&command, &claim, attempt).await
    }

    async fn drive(&self, command: &AdmittedAgentCommand, claim: &CommandClaim) -> CommandAttempt {
        let renew = async {
            loop {
                tokio::time::sleep(RENEW_INTERVAL).await;
                let now = SystemTime::now();
                let renewed = tokio::time::timeout(
                    RENEW_BOUND,
                    self.store.renew_command_claim(
                        claim,
                        rfc3339_at(now),
                        rfc3339_at(now + LEASE_LIFETIME),
                    ),
                )
                .await;
                if !matches!(renewed, Ok(Ok(()))) {
                    return;
                }
            }
        };
        tokio::select! {
            result = tokio::time::timeout(ATTEMPT_BOUND, self.execution.execute(command)) => result.unwrap_or(CommandAttempt::Retry),
            () = renew => CommandAttempt::Retry,
        }
    }

    async fn settle(
        &self,
        original: &AdmittedAgentCommand,
        claim: &CommandClaim,
        attempt: CommandAttempt,
    ) -> Result<Duration, CommandLedgerError> {
        // A timeout/failed response may follow a committed binding. Never infer
        // that execution did not occur, and never release/reject from a stale copy.
        let current = bounded(self.store.get_command(&claim.command_id))
            .await?
            .ok_or(CommandLedgerError::NotFound)?;
        current.validate()?;
        if current.receipt != original.receipt
            || current.key_sha256 != original.key_sha256
            || current.target_turn != original.target_turn
        {
            return Err(CommandLedgerError::Corrupt);
        }
        if current.status.is_terminal() {
            return Ok(Duration::ZERO);
        }
        if current.work.dispatch.is_some() {
            return bounded(self.store.reconcile_command(&claim.command_id))
                .await
                .map(|_| Duration::ZERO);
        }
        current.require_claim(claim, &now_rfc3339())?;
        match attempt {
            CommandAttempt::Finished => Err(CommandLedgerError::Corrupt),
            CommandAttempt::Retry | CommandAttempt::RetryAttachment => {
                bounded(self.store.release_command_claim(claim, now_rfc3339())).await?;
                // Only a confirmed release of an unbound ordinary append can
                // yield this cooldown. A later attempt reacquires all authority.
                let delay = if attempt == CommandAttempt::RetryAttachment
                    && current.receipt.envelope.is_ordinary_append()
                {
                    api_message_recheck_delay(
                        &current.receipt.envelope,
                        &current.receipt.admitted_at,
                        &current.queue_deadline,
                        &now_rfc3339(),
                    )
                } else {
                    Duration::ZERO
                };
                Ok(unclaimed_cooldown(&current, delay))
            }
            CommandAttempt::Reject(code) => {
                bounded(self.store.reject_command(claim, code, now_rfc3339()))
                    .await
                    .map(|_| Duration::ZERO)
            }
        }
    }
}

fn is_channel_message(command: &AdmittedAgentCommand) -> bool {
    matches!(command.receipt.envelope.actor, AgentActor::Channel { .. })
        && command.receipt.envelope.source.kind == AgentSourceKind::Channel
        && matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        )
}

fn unclaimed_recheck_delay(command: &AdmittedAgentCommand) -> Duration {
    if !is_channel_message(command) {
        return UNCLAIMED_RECHECK_DELAY;
    }
    // A transient observation must not add fifteen seconds to a fresh chat
    // message. Age the retry pace from its immutable admission receipt so a
    // sustained outage returns to normal polling, including after restarts.
    let age = crate::seconds_from_rfc3339(&now_rfc3339())
        .zip(crate::seconds_from_rfc3339(&command.receipt.admitted_at))
        .and_then(|(now, admitted)| now.checked_sub(admitted));
    match age {
        Some(0..30) => Duration::from_secs(1),
        Some(30..120) => Duration::from_secs(5),
        _ => UNCLAIMED_RECHECK_DELAY,
    }
}

/// An already authorized API message may be waiting only for startup or the
/// first prompt. Recheck promptly for a bounded age without changing its lane,
/// receipt, queue deadline, or authorization requirements on the next attempt.
fn api_message_recheck_delay(
    envelope: &AgentCommandEnvelope,
    admitted_at: &str,
    deadline: &str,
    now: &str,
) -> Duration {
    if envelope.source.kind != AgentSourceKind::Api
        || !matches!(
            envelope.actor,
            AgentActor::Human { .. } | AgentActor::Api { .. }
        )
        || !matches!(envelope.command, AgentCommand::Message { .. })
    {
        return UNCLAIMED_RECHECK_DELAY;
    }
    let observed = crate::seconds_from_rfc3339(now);
    let age = observed
        .zip(crate::seconds_from_rfc3339(deadline))
        .filter(|(now, deadline)| now < deadline)
        .zip(crate::seconds_from_rfc3339(admitted_at))
        .and_then(|((now, _), admitted)| now.checked_sub(admitted));
    match age {
        Some(0..30) => Duration::from_secs(1),
        Some(30..120) => Duration::from_secs(5),
        _ => UNCLAIMED_RECHECK_DELAY,
    }
}

#[cfg(test)]
#[path = "worker_pacing_tests.rs"]
mod pacing_tests;

/// Preserve the durable deadline while allowing a bounded scheduler to keep
/// scanning past blocked messages. A restart may shorten this process-local
/// cooldown; every attempt still rereads and reauthorizes the command.
fn unclaimed_cooldown(command: &AdmittedAgentCommand, delay: Duration) -> Duration {
    let remaining = crate::seconds_from_rfc3339(&command.queue_deadline)
        .zip(crate::seconds_from_rfc3339(&now_rfc3339()))
        .map(|(deadline, now)| Duration::from_secs(deadline.saturating_sub(now)))
        .unwrap_or_default();
    delay.min(remaining)
}

async fn bounded<T>(
    work: impl std::future::Future<Output = Result<T, CommandLedgerError>>,
) -> Result<T, CommandLedgerError> {
    tokio::time::timeout(IO_BOUND, work)
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
}

//! Durable command driving. A runtime result never proves absence of an atomic
//! dispatch binding; only the authoritative ledger can decide the next action.

use super::*;
use crate::sandbox::store::{Page, PageRequest};
use crate::{ids::AgentCommandId, now_rfc3339, rfc3339_at};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[path = "worker_attempt.rs"]
mod attempt;

const IO_BOUND: Duration = Duration::from_secs(15);
const ATTEMPT_BOUND: Duration = Duration::from_secs(45);
const LEASE_LIFETIME: Duration = Duration::from_secs(30);
const RENEW_INTERVAL: Duration = Duration::from_secs(10);
const RENEW_BOUND: Duration = Duration::from_secs(5);
const UNCLAIMED_RECHECK_DELAY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAttempt {
    Finished,
    Retry,
    Reject(CommandFailureCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandReadiness {
    Ready,
    Retry,
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
    /// Capture ownership for the entire durable attempt. None is reserved for
    /// uncoordinated execution; unavailable ownership must return an error.
    async fn attempt_authority(
        &self,
        _command: &AdmittedAgentCommand,
    ) -> Result<Option<Arc<crate::agent_ownership::Authority>>, CommandPreparationError> {
        Ok(None)
    }

    /// Fresh authority and target checks, with no lifecycle or runtime effects.
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness;
    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError>;
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt;
}

pub struct AgentCommandWorker {
    pub(super) store: Arc<dyn AgentCommandWorkerStore>,
    pub(super) dispatched_recheck_delay: Duration,
    execution: Arc<dyn AgentCommandExecution>,
    holder: String,
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
            holder,
            dispatched_recheck_delay,
        }))
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
            return Ok(Duration::ZERO);
        };
        candidate.validate()?;
        if &candidate.receipt.envelope.command_id != id {
            return Err(CommandLedgerError::Corrupt);
        }
        if candidate.status.is_terminal() {
            return Ok(Duration::ZERO);
        }
        let authority = match tokio::time::timeout(
            IO_BOUND,
            self.execution.attempt_authority(&candidate),
        )
        .await
        {
            Ok(Ok(authority)) => authority,
            // Nonowners must leave even expired queued work untouched. Pacing
            // releases the scheduler slot and reauthorizes on the next attempt.
            _ => return Ok(Duration::from_secs(1)),
        };
        match authority {
            None => self.run_candidate(&candidate).await,
            Some(authority) => {
                if authority.token.agent_id != candidate.receipt.envelope.agent_id {
                    return Err(CommandLedgerError::Corrupt);
                }
                authority
                    .run(self.run_candidate(&candidate))
                    .await
                    .unwrap_or(Ok(Duration::from_secs(1)))
            }
        }
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
    ) -> Result<(), CommandLedgerError> {
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
            return Ok(());
        }
        if current.work.dispatch.is_some() {
            return bounded(self.store.reconcile_command(&claim.command_id))
                .await
                .map(|_| ());
        }
        current.require_claim(claim, &now_rfc3339())?;
        match attempt {
            CommandAttempt::Finished => Err(CommandLedgerError::Corrupt),
            CommandAttempt::Retry => {
                bounded(self.store.release_command_claim(claim, now_rfc3339())).await
            }
            CommandAttempt::Reject(code) => {
                bounded(self.store.reject_command(claim, code, now_rfc3339()))
                    .await
                    .map(|_| ())
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

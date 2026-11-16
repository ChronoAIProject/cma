//! All command mutations, including reconciliation and settlement, run inside
//! the authority captured by run_scheduled. No native input is retried here.
use super::*;

impl AgentCommandWorker {
    pub(super) async fn run_candidate(
        &self,
        candidate: &AdmittedAgentCommand,
    ) -> Result<Duration, CommandLedgerError> {
        let id = &candidate.receipt.envelope.command_id;
        if candidate.work.dispatch.is_some() {
            return bounded(self.store.reconcile_command(id))
                .await
                .map(|command| {
                    super::super::reconcile_pacing::recheck_delay(
                        command.status,
                        self.dispatched_recheck_delay,
                    )
                });
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
            tokio::time::timeout(IO_BOUND, self.execution.check_queued(candidate))
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
            CommandReadiness::Retry => {
                let delay = unclaimed_recheck_delay(candidate);
                if is_channel_message(candidate) {
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
                return Ok(unclaimed_cooldown(candidate, delay));
            }
            CommandReadiness::Reject(code) => {
                return bounded(self.store.reject_unclaimed_command(
                    id,
                    candidate.revision,
                    code,
                    now_rfc3339(),
                ))
                .await
                .map(|()| Duration::ZERO);
            }
        }
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
            if &current != candidate
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
                .await
                .map(|()| Duration::ZERO);
        }
        let attempt = self.drive(&command, &claim).await;
        self.settle(&command, &claim, attempt)
            .await
            .map(|()| Duration::ZERO)
    }
}

use super::*;
use crate::bridge::BridgeManagementLease;

/// Explicit operator acknowledgement, never inferred from elapsed time or an
/// automatic retry policy. Each recovery must be audited in its transaction.
pub struct DuplicateRiskAccepted;

impl OutboxItem {
    /// Persist this transition using CAS with predecessor validation in the same
    /// transaction. Claim expiry before Sending is safe to recover without IO.
    pub fn claim(
        &mut self,
        holder: String,
        now_ms: i64,
        expires_at_ms: i64,
        predecessor: Option<&OutboxItem>,
    ) -> Result<String, OutboxError> {
        self.validate()?;
        if !super::id(&holder)
            || now_ms < self.updated_at_ms
            || expires_at_ms <= now_ms
            || expires_at_ms - now_ms > 60_000
            || self.attempts.len() >= 16
        {
            return Err(OutboxError::Conflict);
        }
        match self.state {
            DeliveryState::Pending => {}
            DeliveryState::RetryWait { retry_at_ms, .. } if retry_at_ms <= now_ms => {}
            DeliveryState::Unknown { .. } => return Err(OutboxError::UnknownOutcome),
            _ => return Err(OutboxError::Conflict),
        }
        self.credential.require_source(&self.source, now_ms)?;
        match (&self.predecessor, predecessor) {
            (None, None) => {}
            (Some(id), Some(prior)) if id == &prior.id => {
                prior.validate()?;
                if prior.source != self.source
                    || prior.sequence + 1 != self.sequence
                    || !matches!(prior.state, DeliveryState::Sent { .. })
                {
                    return Err(OutboxError::PredecessorPending);
                }
            }
            _ => return Err(OutboxError::PredecessorPending),
        }
        let number = self.attempts.len() as u32 + 1;
        let attempt = self.attempt_id(number)?;
        let state = DeliveryState::Claimed {
            attempt: attempt.clone(),
            holder,
            expires_at_ms,
        };
        let mut next = self.clone();
        next.attempts.push(DeliveryAttempt {
            id: attempt.clone(),
            number,
            started_at_ms: now_ms,
            credential: self.credential.clone(),
            state: state.clone(),
        });
        next.transition(state, now_ms)?;
        *self = next;
        Ok(attempt)
    }

    /// Commit Sending before any request bytes. A crash in the small interval
    /// between commit and HTTP remains conservatively Unknown on recovery.
    pub fn begin_send(
        &mut self,
        holder: &str,
        attempt: &str,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        self.validate()?;
        let DeliveryState::Claimed {
            holder: expected,
            attempt: expected_attempt,
            expires_at_ms,
        } = &self.state
        else {
            return Err(OutboxError::Conflict);
        };
        if expected != holder || expected_attempt != attempt || now_ms >= *expires_at_ms {
            return Err(OutboxError::Conflict);
        }
        self.credential.require_source(&self.source, now_ms)?;
        let state = DeliveryState::Sending {
            attempt: attempt.into(),
            expires_at_ms: *expires_at_ms,
        };
        let mut next = self.clone();
        next.transition(state, now_ms)?;
        *self = next;
        Ok(())
    }

    /// A claimed worker can prove no send occurred while validating local or
    /// provider authority. Release that claim with bounded backoff; this method
    /// cannot be used after Sending was committed.
    pub fn defer_before_send(
        &mut self,
        holder: &str,
        attempt: &str,
        reason: DeliveryFailure,
        retry_at_ms: i64,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        self.validate()?;
        if !matches!(&self.state,DeliveryState::Claimed {holder:expected,attempt:current,expires_at_ms}
            if expected==holder && current==attempt && *expires_at_ms>now_ms)
            || retry_at_ms <= now_ms
            || retry_at_ms - now_ms > 86_400_000
        {
            return Err(OutboxError::Conflict);
        }
        let state = if self.attempts.len() >= 16 {
            DeliveryState::Rejected { reason }
        } else if self
            .credential
            .require_source(&self.source, retry_at_ms)
            .is_err()
        {
            DeliveryState::Rejected {
                reason: DeliveryFailure::CredentialExpired,
            }
        } else {
            DeliveryState::RetryWait {
                retry_at_ms,
                reason,
            }
        };
        let mut next = self.clone();
        next.transition(state, now_ms)?;
        *self = next;
        Ok(())
    }

    /// An unclaimed item whose original route expired cannot become deliverable
    /// later. Preserve a terminal receipt instead of leaving it queued forever.
    pub fn reject_expired(&mut self, now_ms: i64) -> Result<(), OutboxError> {
        self.validate()?;
        if !matches!(
            self.state,
            DeliveryState::Pending | DeliveryState::RetryWait { .. }
        ) || self.credential.require_source(&self.source, now_ms) != Err(OutboxError::Expired)
        {
            return Err(OutboxError::Conflict);
        }
        let mut next = self.clone();
        next.transition(
            DeliveryState::Rejected {
                reason: DeliveryFailure::CredentialExpired,
            },
            now_ms,
        )?;
        *self = next;
        Ok(())
    }

    pub fn complete(
        &mut self,
        attempt: &str,
        outcome: DeliveryOutcome,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        self.validate()?;
        let DeliveryState::Sending {
            attempt: current, ..
        } = &self.state
        else {
            return Err(OutboxError::Conflict);
        };
        if current != attempt {
            return Err(OutboxError::Conflict);
        }
        let state = match outcome {
            DeliveryOutcome::Confirmed(receipt) => {
                let valid = super::id(&receipt.nyxid_message_id)
                    && receipt
                        .platform_message_id
                        .as_ref()
                        .is_none_or(|id| super::id(id))
                    && match &self.operation {
                        RelayOperation::InitialReply => true,
                        RelayOperation::Edit {
                            platform_message_id,
                            ..
                        } => receipt.platform_message_id.as_ref() == Some(platform_message_id),
                    };
                if valid {
                    DeliveryState::Sent { receipt }
                } else {
                    DeliveryState::Unknown {
                        attempt: attempt.into(),
                        reason: DeliveryFailure::ReceiptInvalid,
                    }
                }
            }
            DeliveryOutcome::DefinitiveRejection(reason) => DeliveryState::Rejected { reason },
            DeliveryOutcome::RetryableNotSent {
                reason,
                retry_at_ms,
                ..
            } if retry_at_ms > now_ms && retry_at_ms - now_ms <= 86_400_000 => {
                if self
                    .credential
                    .require_source(&self.source, retry_at_ms)
                    .is_err()
                {
                    DeliveryState::Rejected {
                        reason: DeliveryFailure::CredentialExpired,
                    }
                } else {
                    DeliveryState::RetryWait {
                        retry_at_ms,
                        reason,
                    }
                }
            }
            DeliveryOutcome::RetryableNotSent { .. } => return Err(OutboxError::Conflict),
            DeliveryOutcome::Unknown(reason) => DeliveryState::Unknown {
                attempt: attempt.into(),
                reason,
            },
            DeliveryOutcome::RateLimitedUnknown { .. } => DeliveryState::Unknown {
                attempt: attempt.into(),
                reason: DeliveryFailure::RateLimited,
            },
        };
        let mut next = self.clone();
        next.transition(state, now_ms)?;
        *self = next;
        Ok(())
    }

    pub fn recover_expired(&mut self, now_ms: i64) -> Result<(), OutboxError> {
        self.validate()?;
        let state = match &self.state {
            DeliveryState::Claimed { expires_at_ms, .. } if now_ms >= *expires_at_ms => {
                DeliveryState::Pending
            }
            DeliveryState::Sending {
                attempt,
                expires_at_ms,
            } if now_ms >= *expires_at_ms => DeliveryState::Unknown {
                attempt: attempt.clone(),
                reason: DeliveryFailure::WorkerLost,
            },
            _ => return Err(OutboxError::Conflict),
        };
        let mut next = self.clone();
        next.transition(state, now_ms)?;
        *self = next;
        Ok(())
    }

    /// The store must verify/fence the manager lease and audit acknowledgement
    /// with this CAS. Unknown initial sends require a retained matching key:
    /// the original one-time reply token can already have been consumed.
    pub fn retry_unknown(
        &mut self,
        manager: &BridgeManagementLease,
        credential: DeliveryCredentialRef,
        _acknowledgement: DuplicateRiskAccepted,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        self.validate()?;
        manager
            .require_current()
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        self.source.require_bridge(manager.record())?;
        if !matches!(self.state, DeliveryState::Unknown { .. }) {
            return Err(OutboxError::Conflict);
        }
        if now_ms < self.updated_at_ms {
            return Err(OutboxError::Conflict);
        }
        if credential.is_reply_token() {
            return Err(OutboxError::RetainedKeyRequired);
        }
        credential.require_source(&self.source, now_ms)?;
        let mut next = self.clone();
        next.credential = credential;
        next.duplicate_risk = true;
        // Preserve the completed unknown attempt in the audit trail.
        next.state = DeliveryState::Pending;
        next.updated_at_ms = now_ms;
        next.revision = next.revision.checked_add(1).ok_or(OutboxError::Corrupt)?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn abandon(
        &mut self,
        manager: &BridgeManagementLease,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        self.validate()?;
        manager
            .require_current()
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        if manager.record().id != self.source.pin.bridge_id
            || manager.record().owner != self.source.owner
            || matches!(
                self.state,
                DeliveryState::Sending { .. } | DeliveryState::Sent { .. }
            )
        {
            return Err(OutboxError::Conflict);
        }
        let mut next = self.clone();
        if now_ms < next.updated_at_ms {
            return Err(OutboxError::Conflict);
        }
        next.state = DeliveryState::Abandoned {
            actor: manager.actor().clone(),
        };
        next.updated_at_ms = now_ms;
        next.revision = next.revision.checked_add(1).ok_or(OutboxError::Corrupt)?;
        next.validate()?;
        *self = next;
        Ok(())
    }
}

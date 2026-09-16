//! Per-Channel contention budget; the original proof is never refreshed.
use layer2_core::{
    bridge::{invoke::BridgeInvokeError, BridgeError, BridgeSetupBlock},
    sandbox::agent_io::{ChannelCommandAdmissionError, CommandLedgerError},
};
use std::time::Duration;
use tokio::time::Instant;

pub(super) fn normalize_expiry_error(
    authority_deadline: Instant,
    error: ChannelCommandAdmissionError,
) -> ChannelCommandAdmissionError {
    // Expiry can cross the adjacent invoke, connection and ledger checks.
    // Preserve the invoke expiry type without hiding an earlier refusal or
    // accepting a request, and never extend the original authority deadline.
    if Instant::now() >= authority_deadline
        && matches!(
            error,
            ChannelCommandAdmissionError::Invoke(BridgeInvokeError::Bridge(
                BridgeError::ConnectionRequired(BridgeSetupBlock::AuthorityExpired)
            )) | ChannelCommandAdmissionError::Ledger(CommandLedgerError::Conflict)
        )
    {
        return BridgeInvokeError::Unavailable.into();
    }
    error
}

pub(super) struct ChannelAdmissionRetries {
    deadline: Instant,
    attempt: u32,
    jitter: u64,
}

impl ChannelAdmissionRetries {
    pub(super) fn new(authority_deadline: Instant) -> Self {
        Self {
            deadline: authority_deadline.min(Instant::now() + Duration::from_secs(5)),
            attempt: 0,
            jitter: uuid::Uuid::new_v4().as_u128() as u64,
        }
    }

    /// Only waits between complete, proven-retryable attempts. This does not
    /// cancel a transaction or abandon an in-flight commit/reconciliation.
    pub(super) async fn next(&mut self) -> bool {
        let now = Instant::now();
        if now >= self.deadline {
            return false;
        }
        if self.attempt == 0 {
            self.attempt = 1;
            return true;
        }
        let backoff = 5_u64 << (self.attempt - 1).min(4);
        let jitter = self.jitter.rotate_left(self.attempt) & 31;
        let delay = Duration::from_millis(backoff + jitter).min(self.deadline - now);
        tokio::time::sleep(delay).await;
        self.attempt = self.attempt.saturating_add(1);
        Instant::now() < self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChannelCommandAdmissionError as Error;

    #[tokio::test(start_paused = true)]
    async fn only_elapsed_original_authority_normalizes_nested_expiry_errors() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let connection_expired = || {
            Error::Invoke(BridgeInvokeError::Bridge(BridgeError::ConnectionRequired(
                BridgeSetupBlock::AuthorityExpired,
            )))
        };
        assert!(matches!(
            normalize_expiry_error(deadline, connection_expired()),
            Error::Invoke(BridgeInvokeError::Bridge(BridgeError::ConnectionRequired(
                BridgeSetupBlock::AuthorityExpired
            )))
        ));
        assert!(matches!(
            normalize_expiry_error(deadline, Error::Ledger(CommandLedgerError::Conflict)),
            Error::Ledger(CommandLedgerError::Conflict)
        ));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(Instant::now(), deadline);
        for _ in 0..2 {
            for error in [
                connection_expired(),
                Error::Ledger(CommandLedgerError::Conflict),
            ] {
                assert!(matches!(
                    normalize_expiry_error(deadline, error),
                    Error::Invoke(BridgeInvokeError::Unavailable)
                ));
            }
            assert!(matches!(
                normalize_expiry_error(deadline, Error::Invoke(BridgeInvokeError::Revoked)),
                Error::Invoke(BridgeInvokeError::Revoked)
            ));
            assert!(matches!(
                normalize_expiry_error(deadline, Error::SourceMismatch),
                Error::SourceMismatch
            ));
            assert!(matches!(
                normalize_expiry_error(deadline, Error::Unavailable),
                Error::Unavailable
            ));
            tokio::time::advance(Duration::from_secs(1)).await;
        }
    }
}

use super::{Authority, Validity};
use crate::agent_ownership::{OwnershipError, RetirementIntent, Settlement};
use std::sync::atomic::Ordering;
use tokio::time::Instant;

#[derive(Default)]
pub(in crate::agent_ownership) struct CleanupState {
    pub settlement: Option<Settlement>,
    pub legacy_forgotten: bool,
    pub local_quiescent: bool,
    pub observe_next: bool,
}

impl Authority {
    /// Fence the retained exact authority before detachment or any cleanup await.
    pub fn begin_retirement(&self, intent: RetirementIntent) -> Result<(), OwnershipError> {
        let mut accepted = false;
        self.validity.send_modify(|state| {
            state.observe(Instant::now());
            match *state {
                Validity::Live { .. } => {
                    *state = Validity::Retiring { intent };
                    accepted = true;
                }
                Validity::Unpublished
                | Validity::Retiring { .. }
                | Validity::RetryExpired { .. }
                | Validity::ReceiverCancelled { .. } => accepted = true,
                _ => (),
            }
        });
        if accepted {
            self.retiring.store(true, Ordering::Release);
            Ok(())
        } else {
            Err(OwnershipError::Stale)
        }
    }

    pub fn retirement_pending(&self) -> bool {
        matches!(
            self.observe_validity(),
            Validity::Unpublished
                | Validity::Retiring { .. }
                | Validity::RetryExpired { .. }
                | Validity::ReceiverCancelled { .. }
        )
    }
}

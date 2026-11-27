//! Distinguish dispatch evidence from a platform's terminal receipt.
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerDispatch {
    NotDispatched,
    Attempted,
    Indeterminate,
}

const STARTED: u8 = 1;
const CLOSED: u8 = 2;

#[derive(Default)]
pub(super) struct DispatchBoundary(AtomicU8);

impl DispatchBoundary {
    pub(super) fn is_closed(&self) -> bool {
        self.0.load(Ordering::SeqCst) & CLOSED != 0
    }

    /// Cancellation and the first platform attempt share the same atomic
    /// boundary. A timed-out waiter can prove no dispatch only if it wins.
    pub(super) fn commit(&self) -> bool {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                (state & CLOSED == 0).then_some(state | STARTED)
            })
            .is_ok()
    }

    pub(super) fn close(&self) -> AnswerDispatch {
        if self.0.fetch_or(CLOSED, Ordering::SeqCst) & STARTED == 0 {
            AnswerDispatch::NotDispatched
        } else {
            AnswerDispatch::Attempted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_boundary_refuses_new_dispatch_and_preserves_prior_evidence() {
        let before = DispatchBoundary::default();
        assert_eq!(before.close(), AnswerDispatch::NotDispatched);
        assert!(!before.commit());
        assert_eq!(before.close(), AnswerDispatch::NotDispatched);

        let after = DispatchBoundary::default();
        assert!(after.commit());
        assert!(after.commit());
        assert_eq!(after.close(), AnswerDispatch::Attempted);
        assert!(!after.commit());
        assert_eq!(after.close(), AnswerDispatch::Attempted);
    }
}

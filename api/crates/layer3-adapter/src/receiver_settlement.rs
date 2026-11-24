use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverSettlementFailure {
    Deadline,
    Unavailable,
    IncompatibleProtocol,
    Invalid,
    AuthorityLost,
    Unclassified,
}

impl ReceiverSettlementFailure {
    pub fn is_transient(self) -> bool {
        match self {
            Self::Deadline | Self::Unavailable => true,
            Self::IncompatibleProtocol
            | Self::Invalid
            | Self::AuthorityLost
            | Self::Unclassified => false,
        }
    }
}

/// A terminal validation result survives cancellation and diagnostic awaits.
/// Transient observations are never retained here or reused by a later attempt.
#[derive(Clone, Default)]
pub struct ReceiverSettlementWatch(Arc<OnceLock<ReceiverSettlementFailure>>);

impl ReceiverSettlementWatch {
    pub fn record(&self, failure: ReceiverSettlementFailure) {
        if !failure.is_transient() {
            let _ = self.0.set(failure);
        }
    }

    pub fn terminal(&self) -> Option<ReceiverSettlementFailure> {
        self.0.get().copied()
    }
}

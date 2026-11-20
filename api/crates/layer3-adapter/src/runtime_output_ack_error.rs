use crate::ingress_budget::{diagnostic::text, AdmissionRefusal};
use crate::AdapterError;

/// Fixed ACK failures can be retained outside the initiating work scope.
/// No variant contains a heap allocation or implies durable consumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmittedAckError {
    #[error("runtime acknowledgement identity exceeds its bound")]
    IdentityTooLarge,
    #[error("runtime acknowledgement queue is poisoned")]
    Poisoned,
    #[error("runtime acknowledgement queue is full")]
    Full,
    #[error("runtime acknowledgement link ended")]
    Closed,
    #[error("acknowledgement capacity must be positive")]
    InvalidCapacity,
    #[error("runtime acknowledgement has no admitted queue")]
    OwnershipRequired,
    #[error(transparent)]
    Admission(#[from] AdmissionRefusal),
}

impl AdmittedAckError {
    pub(super) fn legacy(self) -> AdapterError {
        match self {
            Self::IdentityTooLarge => {
                AdapterError::Rejected(text("runtime acknowledgement identity exceeds its bound"))
            }
            Self::Poisoned => {
                AdapterError::Unavailable(text("runtime acknowledgement queue is poisoned"))
            }
            Self::Full => AdapterError::AtCapacity(text("runtime acknowledgement queue is full")),
            Self::Closed => AdapterError::Unavailable(text("runtime acknowledgement link ended")),
            Self::InvalidCapacity => {
                AdapterError::Rejected(text("acknowledgement capacity must be positive"))
            }
            Self::OwnershipRequired => {
                AdapterError::Unsupported(text("runtime acknowledgement has no admitted queue"))
            }
            Self::Admission(refusal) => refusal.into(),
        }
    }
}

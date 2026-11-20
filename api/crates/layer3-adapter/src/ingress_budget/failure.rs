//! Static admission failures and immutable ownership of runtime error payloads.

use std::fmt;

use super::SharedOwned;
use crate::AdapterError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefusal {
    #[error("ingress allocation capacity exhausted during {phase}")]
    AtCapacity { phase: &'static str },
    #[error("ingress allocation plan exceeds its supported bounds during {phase}")]
    InvalidPlan { phase: &'static str },
}

impl From<AdmissionRefusal> for AdapterError {
    fn from(refusal: AdmissionRefusal) -> Self {
        match refusal {
            AdmissionRefusal::AtCapacity { .. } => super::diagnostic::make(
                Self::AtCapacity,
                format_args!("ingress allocation capacity exhausted"),
            ),
            AdmissionRefusal::InvalidPlan { .. } => super::sizing::invalid(),
        }
    }
}

/// Admission refusal needs no heap storage. Runtime errors retain their owner
/// through classification, cloning and the final caller's result lifetime.
#[derive(Clone, Debug)]
pub enum AdmittedError {
    Refused(AdmissionRefusal),
    Unavailable(&'static str),
    Unsupported(&'static str),
    Adapter(SharedOwned<AdapterError>),
}

impl AdmittedError {
    pub fn belongs_to(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        match self {
            Self::Adapter(error) => error.belongs_to(policy),
            Self::Refused(_) | Self::Unavailable(_) | Self::Unsupported(_) => true,
        }
    }

    pub fn as_adapter(&self) -> Option<&AdapterError> {
        match self {
            Self::Adapter(error) => Some(error),
            Self::Refused(_) | Self::Unavailable(_) | Self::Unsupported(_) => None,
        }
    }

    pub fn refusal(&self) -> Option<AdmissionRefusal> {
        match self {
            Self::Refused(refusal) => Some(*refusal),
            Self::Adapter(_) | Self::Unavailable(_) | Self::Unsupported(_) => None,
        }
    }

    pub fn requested_bytes(&self) -> usize {
        match self {
            Self::Refused(_) | Self::Unavailable(_) | Self::Unsupported(_) => 0,
            Self::Adapter(error) => error.requested_bytes(),
        }
    }
}

impl From<AdmissionRefusal> for AdmittedError {
    fn from(refusal: AdmissionRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<SharedOwned<AdapterError>> for AdmittedError {
    fn from(error: SharedOwned<AdapterError>) -> Self {
        Self::Adapter(error)
    }
}

impl fmt::Display for AdmittedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Unavailable(detail) => write!(f, "runtime unavailable: {detail}"),
            Self::Unsupported(detail) => write!(f, "runtime capability is not supported: {detail}"),
            Self::Adapter(error) => fmt::Display::fmt(&**error, f),
        }
    }
}

impl std::error::Error for AdmittedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.as_adapter().map(|error| error as _)
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;

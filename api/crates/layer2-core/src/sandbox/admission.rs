//! Server-attributed admission causes and typed service refusals.

use crate::admission::{AdmissionCause, AdmissionRefusal};

use super::store::{AgentAdmission, SandboxAdmission, WorkspaceAdmission};
use super::{AgentOrigin, SandboxOrigin, WorkspaceOrigin};

impl SandboxAdmission {
    pub fn admission_cause(&self) -> AdmissionCause {
        if self.binding.is_some() || !matches!(self.sandbox.origin, SandboxOrigin::Manual) {
            AdmissionCause::Continuation
        } else {
            AdmissionCause::Fresh
        }
    }
}

impl WorkspaceAdmission {
    pub fn admission_cause(&self) -> AdmissionCause {
        if self.binding.is_some() || !matches!(self.workspace.origin, WorkspaceOrigin::Manual) {
            AdmissionCause::Continuation
        } else {
            AdmissionCause::Fresh
        }
    }
}

impl AgentAdmission {
    pub fn admission_cause(&self) -> AdmissionCause {
        if self.binding.is_some() || !matches!(self.origin, AgentOrigin::Manual) {
            AdmissionCause::Continuation
        } else {
            AdmissionCause::Fresh
        }
    }
}

macro_rules! admission_error {
    ($($error:ty => $capacity:expr),+ $(,)?) => {$(
        impl From<AdmissionRefusal> for $error {
            fn from(refusal: AdmissionRefusal) -> Self {
                match refusal {
                    AdmissionRefusal::Budget(reason) => Self::Budget(reason),
                    AdmissionRefusal::Paused => Self::AdmissionPaused,
                    AdmissionRefusal::Capacity(kind) => ($capacity)(kind),
                }
            }
        }
    )+};
}

admission_error!(
    super::store::SandboxStoreError => |kind| match kind {
        crate::admission::CapacityKind::RetainedHomes => Self::GlobalCapacityReached,
        crate::admission::CapacityKind::ActiveSessions => Self::SessionCapacityUnavailable,
        kind => Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    },
    super::SandboxServiceError => |kind| match kind {
        crate::admission::CapacityKind::RetainedHomes => Self::GlobalCapacityReached,
        kind => Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    },
    super::SandboxLifecycleCommandError => |kind| Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    super::control::SandboxControlServiceError => |_| Self::Store(super::control::SandboxControlError::Unavailable),
    super::control::SandboxControlError => |_| Self::Unavailable,
    super::AgentServiceError => |kind| Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    super::WorkspaceServiceError => |kind| Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    super::AgentOperationError => |kind| match kind {
        crate::admission::CapacityKind::ActiveSessions => Self::SessionCapacityUnavailable,
        kind => Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    },
    super::agent_io::CommandLedgerError => |kind| match kind {
        crate::admission::CapacityKind::ActiveSessions => Self::SessionCapacityUnavailable,
        _ => Self::Unavailable,
    },
    crate::trigger_domain::LaunchServiceError => |kind| match kind {
        crate::admission::CapacityKind::RetainedHomes => Self::GlobalCapacityReached,
        kind => Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    },
    crate::trigger_domain::LaunchStoreError => |kind| match kind {
        crate::admission::CapacityKind::RetainedHomes => Self::GlobalCapacityReached,
        kind => Self::Unavailable(AdmissionRefusal::Capacity(kind).to_string()),
    },
);

#[cfg(test)]
mod capacity_tests {
    use super::*;
    use crate::admission::CapacityKind;

    #[test]
    fn capacity_refusals_preserve_the_runtime_domain_kind() {
        assert_eq!(
            crate::trigger_domain::LaunchStoreError::from(AdmissionRefusal::Capacity(
                CapacityKind::RetainedHomes
            )),
            crate::trigger_domain::LaunchStoreError::GlobalCapacityReached,
        );
        assert_eq!(
            crate::trigger_domain::LaunchServiceError::from(AdmissionRefusal::Capacity(
                CapacityKind::RetainedHomes
            )),
            crate::trigger_domain::LaunchServiceError::GlobalCapacityReached,
        );
        assert_eq!(
            super::super::store::SandboxStoreError::from(AdmissionRefusal::Capacity(
                CapacityKind::RetainedHomes
            )),
            super::super::store::SandboxStoreError::GlobalCapacityReached,
        );
        assert_eq!(
            super::super::store::SandboxStoreError::from(AdmissionRefusal::Capacity(
                CapacityKind::ActiveSessions
            )),
            super::super::store::SandboxStoreError::SessionCapacityUnavailable,
        );
        assert_eq!(
            super::super::AgentOperationError::from(AdmissionRefusal::Capacity(
                CapacityKind::ActiveSessions
            )),
            super::super::AgentOperationError::SessionCapacityUnavailable,
        );
        assert_eq!(
            super::super::agent_io::CommandLedgerError::from(AdmissionRefusal::Capacity(
                CapacityKind::ActiveSessions
            )),
            super::super::agent_io::CommandLedgerError::SessionCapacityUnavailable,
        );
    }
}

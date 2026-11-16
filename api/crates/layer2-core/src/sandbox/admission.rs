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
    ($($error:ty),+ $(,)?) => {$(
        impl From<AdmissionRefusal> for $error {
            fn from(refusal: AdmissionRefusal) -> Self {
                match refusal {
                    AdmissionRefusal::Budget(reason) => Self::Budget(reason),
                    AdmissionRefusal::Paused => Self::AdmissionPaused,
                }
            }
        }
    )+};
}

admission_error!(
    super::store::SandboxStoreError,
    super::SandboxServiceError,
    super::SandboxLifecycleCommandError,
    super::control::SandboxControlServiceError,
    super::control::SandboxControlError,
    super::AgentServiceError,
    super::WorkspaceServiceError,
    super::AgentOperationError,
    super::agent_io::CommandLedgerError,
    crate::trigger_domain::LaunchServiceError,
    crate::trigger_domain::LaunchStoreError,
);

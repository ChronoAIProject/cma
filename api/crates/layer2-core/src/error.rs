//! Failures shared by retained core foundations.

use crate::InvalidPressurePolicy;

/// A protocol-independent core refusal or dependency failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("saving admission ceilings is not available yet")]
    AdmissionCeilingsWritesUnavailable,
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("admission capacity is unavailable: {0}")]
    AdmissionCapacity(crate::admission::CapacityKind),
    /// The compatible-reader rollout has not enabled admission control writes.
    #[error("saving admission control is not available yet")]
    AdmissionControlWritesUnavailable,
    /// The compatible-reader rollout has not enabled model budget settings writes.
    #[error("saving model budget settings is not available yet")]
    ModelBudgetWritesUnavailable,
    /// The caller lacks the required authority.
    #[error("forbidden")]
    Forbidden,
    /// A well-formed request contains an unusable value.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// The operation cannot run in the resource's current state.
    #[error("invalid state for operation: {0}")]
    InvalidState(String),
    /// A runtime dependency could not be reached.
    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),
    /// A runtime answered but refused the operation.
    #[error("runtime refused: {0}")]
    RuntimeRefused(String),
    /// A service required by the request was not granted.
    #[error("service not granted: {0}")]
    ServiceNotGranted(String),
    /// The identity service catalogue could not be read.
    #[error("service catalogue unavailable: {0}")]
    ServiceCatalogUnavailable(String),
    /// The skill catalogue could not be read completely.
    #[error("skill catalogue unavailable: {0}")]
    SkillCatalogUnavailable(String),
    /// A pressure policy violates its hysteresis contract.
    #[error("invalid pressure policy: {0}")]
    InvalidPressurePolicy(#[from] InvalidPressurePolicy),
}

impl CoreError {
    /// Stable machine-readable code for public-interface projections.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AdmissionPaused => "admission_paused",
            Self::AdmissionCapacity(kind) => kind.code(),
            Self::AdmissionCeilingsWritesUnavailable => "admission_ceilings_writes_unavailable",
            Self::AdmissionControlWritesUnavailable => "admission_control_writes_unavailable",
            Self::ModelBudgetWritesUnavailable => "model_budget_writes_unavailable",
            Self::Forbidden => "forbidden",
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidState(_) => "invalid_state",
            Self::RuntimeUnavailable(_) => "runtime_unavailable",
            Self::RuntimeRefused(_) => "runtime_refused",
            Self::ServiceNotGranted(_) => "service_not_granted",
            Self::ServiceCatalogUnavailable(_) => "service_catalogue_unavailable",
            Self::SkillCatalogUnavailable(_) => "skill_catalogue_unavailable",
            Self::InvalidPressurePolicy(_) => "invalid_pressure_policy",
        }
    }
}

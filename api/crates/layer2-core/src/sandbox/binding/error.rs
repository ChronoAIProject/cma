use crate::sandbox::store::SandboxStoreError;

use super::unavailable;

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("retained Sandbox capacity is temporarily unavailable")]
    GlobalCapacityReached,
    #[error("forbidden")]
    Forbidden,
    #[error("binding not found")]
    NotFound,
    #[error("invalid binding request: {0}")]
    InvalidRequest(String),
    #[error("binding changed or is not ready")]
    Conflict,
    #[error(
        "skill {id:?} is installed at version {installed:?}; changing it to version {requested:?} is unsupported for an existing binding"
    )]
    SkillVersionUpdateUnsupported {
        id: String,
        installed: String,
        requested: String,
    },
    #[error("This binding's sandbox or workspace is missing or removed. Reconnect the channel to an available agent.")]
    ResourcesUnavailable,
    #[error("This binding update cannot preserve the existing Agent conversation and home. Service scope, skills, prompt, model, environment and conversation affinity remain pinned; update the existing resources through their supported controls. The binding and its retained work have not been changed. Rotate a credential with the current profile before changing metadata.")]
    ReconfigurationUnsupported,
    #[error("binding dependency unavailable: {0}")]
    Unavailable(String),
    #[error("binding data is not usable: {0}")]
    Corrupt(String),
}

impl From<SandboxStoreError> for BindingError {
    fn from(error: SandboxStoreError) -> Self {
        match error {
            SandboxStoreError::AdmissionPaused => Self::AdmissionPaused,
            SandboxStoreError::GlobalCapacityReached => Self::GlobalCapacityReached,
            SandboxStoreError::NotFound => Self::NotFound,
            SandboxStoreError::Conflict | SandboxStoreError::Duplicate => Self::Conflict,
            SandboxStoreError::Corrupt(detail) => Self::Corrupt(detail),
            other => unavailable(other),
        }
    }
}

impl From<crate::CoreError> for BindingError {
    fn from(error: crate::CoreError) -> Self {
        match error {
            crate::CoreError::AdmissionPaused => Self::AdmissionPaused,
            crate::CoreError::AdmissionCapacity(crate::admission::CapacityKind::RetainedHomes) => {
                Self::GlobalCapacityReached
            }
            crate::CoreError::InvalidRequest(detail) => Self::InvalidRequest(detail),
            crate::CoreError::InvalidState(_) => Self::Conflict,
            other => unavailable(other),
        }
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn binding_store_capacity_refusal_preserves_its_domain_type() {
        assert!(matches!(
            BindingError::from(SandboxStoreError::GlobalCapacityReached),
            BindingError::GlobalCapacityReached,
        ));
        assert!(matches!(
            BindingError::from(SandboxStoreError::AdmissionPaused),
            BindingError::AdmissionPaused,
        ));
    }

    #[test]
    fn binding_custody_capacity_refusal_preserves_only_the_home_capacity_kind() {
        use crate::admission::CapacityKind;
        assert!(matches!(
            BindingError::from(crate::CoreError::AdmissionCapacity(
                CapacityKind::RetainedHomes
            )),
            BindingError::GlobalCapacityReached,
        ));
        for kind in [CapacityKind::Registrations, CapacityKind::ActiveSessions] {
            assert!(matches!(
                BindingError::from(crate::CoreError::AdmissionCapacity(kind)),
                BindingError::Unavailable(_),
            ));
        }
        assert_eq!(
            crate::CoreError::AdmissionCapacity(CapacityKind::RetainedHomes).code(),
            "sandbox_capacity_unavailable",
        );
        assert!(matches!(
            BindingError::from(crate::CoreError::AdmissionPaused),
            BindingError::AdmissionPaused,
        ));
    }
}

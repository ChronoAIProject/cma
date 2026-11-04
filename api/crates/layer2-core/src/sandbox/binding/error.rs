use crate::sandbox::store::SandboxStoreError;

use super::unavailable;

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
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
            SandboxStoreError::NotFound => Self::NotFound,
            SandboxStoreError::Conflict | SandboxStoreError::Duplicate => Self::Conflict,
            SandboxStoreError::Corrupt(detail) => Self::Corrupt(detail),
            other => unavailable(other),
        }
    }
}

use crate::SubjectId;

/// Checks owner-held renewable authority without exposing any credential to Core.
#[async_trait::async_trait]
pub trait SandboxCredentialPrerequisite: Send + Sync {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxCredentialPrerequisiteError {
    #[error("renewable Sandbox credential setup is required")]
    SetupRequired,
    #[error("Sandbox credential prerequisite unavailable: {0}")]
    Unavailable(String),
}

/// Explicit fixed authority for isolated compositions with synthetic credentials.
#[derive(Debug, Clone, Copy)]
pub enum FixedSandboxCredentialPrerequisite {
    Ready,
    SetupRequired,
}

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for FixedSandboxCredentialPrerequisite {
    async fn check(&self, _owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        match self {
            Self::Ready => Ok(()),
            Self::SetupRequired => Err(SandboxCredentialPrerequisiteError::SetupRequired),
        }
    }
}

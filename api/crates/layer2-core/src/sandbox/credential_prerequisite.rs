use crate::{ServiceId, SubjectId};

/// Checks owner-held renewable authority without exposing any credential to Core.
#[async_trait::async_trait]
pub trait SandboxCredentialPrerequisite: Send + Sync {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError>;

    async fn check_sandbox(
        &self,
        sandbox: &super::Sandbox,
    ) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.check_services(
            &sandbox.owner,
            &sandbox
                .profile
                .content
                .connection_services_for_grants(&sandbox.credential.effective_reaches()),
        )
        .await
    }

    async fn connection_services(
        &self,
        profile: &crate::profiles::SandboxProfileContent,
    ) -> Result<Vec<ServiceId>, SandboxCredentialPrerequisiteError> {
        Ok(profile.connection_services())
    }

    /// Production checks both renewable identity authority and each actual
    /// integration/model connection before admitting or reopening resources.
    async fn check_services(
        &self,
        owner: &SubjectId,
        _services: &[ServiceId],
    ) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.check(owner).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxCredentialPrerequisiteError {
    #[error("renewable Sandbox credential setup is required")]
    SetupRequired,
    #[error("required connections have not been verified")]
    ConnectionsRequired { services: Vec<ServiceId> },
    #[error("Sandbox credential prerequisite unavailable: {0}")]
    Unavailable(String),
}

impl SandboxCredentialPrerequisiteError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SetupRequired => "sandbox_credential_setup_required",
            Self::ConnectionsRequired { .. } => "connections_required",
            Self::Unavailable(_) => "connection_unavailable",
        }
    }
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

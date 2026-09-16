//! Distinguish an explicit credential refusal from unavailable provider evidence.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ProviderVerificationError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unavailable(String),
}

impl ProviderVerificationError {
    pub(super) fn command_error(self) -> layer2_core::sandbox::agent_io::CommandAuthorizationError {
        use layer2_core::sandbox::agent_io::CommandAuthorizationError;
        match self {
            Self::Rejected(_) => CommandAuthorizationError::Revoked,
            Self::Unavailable(_) => CommandAuthorizationError::Unavailable,
        }
    }
}

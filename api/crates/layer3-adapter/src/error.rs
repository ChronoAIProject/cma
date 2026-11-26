/// Convenient adapter result.
pub type Result<T> = std::result::Result<T, AdapterError>;

/// The capacity dimension shared by adapter implementations and upper layers.
/// This type carries no provider or transport vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCapacityKind {
    Stream,
    Request,
}

/// Exact image-contract admission failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime image {image} has contract label {found:?}; expected exactly {expected}")]
pub struct RuntimeContractMismatch {
    pub image: String,
    pub expected: String,
    pub found: Option<String>,
}

/// A provider-neutral adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// A prompt was refused before its frame reached the transport writer.
    #[error("runtime prompt was not written: {0}")]
    PromptNotWritten(String),
    #[error("input authority temporarily unavailable: {0}")]
    InputAuthorityUnavailable(String),
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
    #[error("the desktop has stopped recovering")]
    DesktopFailed,
    /// A create readiness observation; it does not prove runtime loss or permit replacement.
    #[error("native claim is not ready: {0}")]
    NativeClaimNotReady(crate::agent_sandbox::NativeClaimGuardReason),
    #[error("runtime rejected request: {0}")]
    Rejected(String),
    /// The admitted identity is intact, but the provider's current spec differs.
    #[error("Sandbox spec differs from its admitted configuration")]
    AdmittedSandboxSpecDrift,
    #[error("native pool or template differs from its recorded configuration")]
    NativePoolConfigurationDrift,
    #[error("recorded native definitions absent (pool={pool}, template={template})")]
    NativePoolDefinitionAbsent { pool: bool, template: bool },
    /// Current authority no longer permits the operation.
    #[error("current access authority was revoked")]
    AccessRevoked,
    #[error("skill upgrade refused: {0}")]
    SkillUpgradeRefused(crate::AgentSkillUpgradeRefusal),
    #[error("runtime rejected conflicting operation identity: {0}")]
    OperationIdentityConflict(String),
    /// Reachable runtime whose retained mutation ledger cannot admit work.
    #[error("runtime ledger hold: {0}")]
    RuntimeLedgerHold(String),
    /// Repeated acknowledged startup failures; original work remains pending.
    #[error("runtime startup hold: {0}")]
    RuntimeStartupHold(crate::RuntimeStartupFailure),
    #[error("no capacity for the requested runtime: {0}")]
    AtCapacity(String),
    #[error(transparent)]
    RuntimeCapacity(#[from] crate::agent_sandbox::RuntimeCapacityFailure),
    #[error("runtime capability is not supported: {0}")]
    Unsupported(String),
    #[error(transparent)]
    RuntimeContractMismatch(#[from] RuntimeContractMismatch),
}

impl AdapterError {
    pub(crate) fn is_native_pool_binding_hold(&self) -> bool {
        matches!(
            self,
            Self::NativePoolConfigurationDrift | Self::NativePoolDefinitionAbsent { .. }
        )
    }

    pub(crate) const NATIVE_HOME_IDENTITY_REFUSAL: &str =
        "native home identity changed or disappeared";

    #[must_use]
    pub fn is_native_home_identity_refusal(&self) -> bool {
        matches!(self, Self::Rejected(detail) if detail == Self::NATIVE_HOME_IDENTITY_REFUSAL)
    }

    /// Add diagnostic context without changing the failure category.
    #[must_use]
    pub fn context(self, context: &str) -> Self {
        self.context_formatted(format_args!("{context}"))
    }

    pub(crate) fn context_formatted(self, detail: std::fmt::Arguments<'_>) -> Self {
        use crate::ingress_budget::diagnostic::context;
        match self {
            Self::PromptNotWritten(message) => context(Self::PromptNotWritten, message, detail),
            Self::InputAuthorityUnavailable(message) => {
                context(Self::InputAuthorityUnavailable, message, detail)
            }
            Self::Unavailable(message) => context(Self::Unavailable, message, detail),
            Self::DesktopFailed => Self::DesktopFailed,
            held @ Self::RuntimeStartupHold(_) => held,
            guarded @ Self::NativeClaimNotReady(_) => guarded,
            Self::Rejected(message) => context(Self::Rejected, message, detail),
            Self::AdmittedSandboxSpecDrift => Self::AdmittedSandboxSpecDrift,
            Self::NativePoolConfigurationDrift => Self::NativePoolConfigurationDrift,
            absent @ Self::NativePoolDefinitionAbsent { .. } => absent,
            Self::AccessRevoked => Self::AccessRevoked,
            refusal @ Self::SkillUpgradeRefused(_) => refusal,
            Self::OperationIdentityConflict(message) => {
                context(Self::OperationIdentityConflict, message, detail)
            }
            Self::RuntimeLedgerHold(message) => context(Self::RuntimeLedgerHold, message, detail),
            capacity @ Self::RuntimeCapacity(_) => capacity,
            Self::AtCapacity(message) => context(Self::AtCapacity, message, detail),
            Self::Unsupported(message) => context(Self::Unsupported, message, detail),
            mismatch @ Self::RuntimeContractMismatch(_) => mismatch,
        }
    }

    /// Only silence or loss of reachability proves that a runtime disappeared.
    #[must_use]
    pub fn is_runtime_loss(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::RuntimeStartupHold(_))
    }

    pub(crate) fn unexpected_reply(operation: &str) -> Self {
        crate::ingress_budget::diagnostic::make_preserving_category(
            Self::Rejected,
            format_args!(
                "runtime returned a reply for a different operation while handling {operation}"
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::AdapterError;

    #[test]
    fn access_revoked_keeps_its_type_after_diagnostic_context() {
        let error = AdapterError::AccessRevoked
            .context("credential issuance")
            .context("current authority");
        assert_eq!(error, AdapterError::AccessRevoked);
        assert!(!error.is_runtime_loss());
    }
}

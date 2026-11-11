/// Convenient adapter result.
pub type Result<T> = std::result::Result<T, AdapterError>;

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
    #[error("runtime rejected request: {0}")]
    Rejected(String),
    #[error("runtime rejected conflicting operation identity: {0}")]
    OperationIdentityConflict(String),
    /// Reachable runtime whose retained mutation ledger cannot admit work.
    #[error("runtime ledger hold: {0}")]
    RuntimeLedgerHold(String),
    #[error("no capacity for the requested runtime: {0}")]
    AtCapacity(String),
    #[error("runtime capability is not supported: {0}")]
    Unsupported(String),
    #[error(transparent)]
    RuntimeContractMismatch(#[from] RuntimeContractMismatch),
}

impl AdapterError {
    /// Add diagnostic context without changing the failure category.
    #[must_use]
    pub fn context(self, context: &str) -> Self {
        match self {
            Self::PromptNotWritten(message) => {
                Self::PromptNotWritten(format!("{context}: {message}"))
            }
            Self::InputAuthorityUnavailable(message) => {
                Self::InputAuthorityUnavailable(format!("{context}: {message}"))
            }
            Self::Unavailable(message) => Self::Unavailable(format!("{context}: {message}")),
            Self::Rejected(message) => Self::Rejected(format!("{context}: {message}")),
            Self::OperationIdentityConflict(message) => {
                Self::OperationIdentityConflict(format!("{context}: {message}"))
            }
            Self::RuntimeLedgerHold(message) => {
                Self::RuntimeLedgerHold(format!("{context}: {message}"))
            }
            Self::AtCapacity(message) => Self::AtCapacity(format!("{context}: {message}")),
            Self::Unsupported(message) => Self::Unsupported(format!("{context}: {message}")),
            mismatch @ Self::RuntimeContractMismatch(_) => mismatch,
        }
    }

    /// Only silence or loss of reachability proves that a runtime disappeared.
    #[must_use]
    pub fn is_runtime_loss(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    pub(crate) fn unexpected_reply(operation: &str) -> Self {
        Self::Rejected(format!(
            "runtime returned a reply for a different operation while handling {operation}"
        ))
    }
}

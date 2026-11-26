use super::SandboxStoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentWorkClaimDetail {
    RetryExhausted,
    Read,
    Driver,
    PublicationCustody,
}

/// The store error preserves retry semantics. The detail is a closed diagnostic.
pub struct AgentWorkClaimError {
    pub store_error: SandboxStoreError,
    pub detail: Option<AgentWorkClaimDetail>,
}

impl AgentWorkClaimError {
    pub fn with_detail(store_error: SandboxStoreError, detail: AgentWorkClaimDetail) -> Self {
        Self {
            store_error,
            detail: Some(detail),
        }
    }
}

impl From<SandboxStoreError> for AgentWorkClaimError {
    fn from(store_error: SandboxStoreError) -> Self {
        Self {
            store_error,
            detail: None,
        }
    }
}

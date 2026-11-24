//! Keep transactional helper failures typed until their owning retry loop.
use super::ApplyError;
use layer2_core::sandbox::SandboxStoreError;
use mongodb::error::Error;

pub(in crate::store) enum HelperError {
    Domain(SandboxStoreError),
    Driver(Error),
}

impl HelperError {
    pub(in crate::store) fn into_store_error(
        self,
        driver: fn(Error) -> SandboxStoreError,
    ) -> SandboxStoreError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(error) => driver(error),
        }
    }
}

impl From<SandboxStoreError> for HelperError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}

impl From<HelperError> for ApplyError {
    fn from(error: HelperError) -> Self {
        match error {
            HelperError::Domain(error) => Self::Domain(error),
            HelperError::Driver(error) => Self::Driver(error),
        }
    }
}

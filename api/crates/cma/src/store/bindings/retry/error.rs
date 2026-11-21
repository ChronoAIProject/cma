//! Keep known-aborted driver failures distinct from credential/domain refusals.
use layer2_core::CoreError;

pub(in crate::store) enum BindingTransactionError {
    Domain(CoreError),
    Driver(mongodb::error::Error),
}

impl BindingTransactionError {
    pub(in crate::store) fn into_core(
        self,
        unavailable: impl FnOnce(mongodb::error::Error) -> CoreError,
    ) -> CoreError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(error) => unavailable(error),
        }
    }
}

impl From<CoreError> for BindingTransactionError {
    fn from(error: CoreError) -> Self {
        Self::Domain(error)
    }
}

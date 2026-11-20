//! Transaction reads retain driver labels separately from domain and admission failures.
use layer2_core::sandbox::ingress_ownership::{
    AdmissionRefusal, AdmittedError, IngressWorkScope, OwnedWork,
};
use layer2_core::sandbox::store::SandboxStoreError;

use super::layout::LayoutError;
use super::row::{ReadError, RowError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadFailure {
    #[error(transparent)]
    Driver(mongodb::error::Error),
    #[error(transparent)]
    Domain(SandboxStoreError),
    #[error("{0}")]
    InvalidPage(&'static str),
}

pub(crate) fn finish<T>(
    result: Result<T, ReadError<mongodb::error::Error>>,
    scope: &IngressWorkScope,
) -> Result<OwnedWork<Result<T, ReadFailure>>, AdmittedError> {
    match result {
        Ok(value) => Ok(scope.initialize_owned(|| Ok(value))),
        Err(ReadError::Source(error)) => Ok(error.map(|error| Err(ReadFailure::Driver(error)))),
        Err(ReadError::Row(RowError::Domain(error))) => {
            Ok(error.map(|error| Err(ReadFailure::Domain(error))))
        }
        Err(ReadError::Row(RowError::Refused(LayoutError::Capacity))) => {
            Err(AdmissionRefusal::AtCapacity {
                phase: "transaction row",
            }
            .into())
        }
        // A preflight limit/unsupported BSON shape is not a domain validator's
        // corruption finding and must never authorize quarantine.
        Err(ReadError::Row(RowError::Refused(LayoutError::Structure))) => Err(
            AdmittedError::Unsupported("transaction BSON layout is unsupported"),
        ),
    }
}

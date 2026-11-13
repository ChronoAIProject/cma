//! Durable replica participation around the existing gateway record journals.
mod backfill;
pub mod config;
pub(crate) mod drain;
mod drain_cli;
pub(crate) mod effect_recovery;
mod effect_roots;
pub mod effects;
pub(crate) mod membership;
pub mod partition;
pub(crate) mod partition_discovery;
mod partition_model;
mod partition_mutation;
mod partition_queue;
mod protocol;
pub(crate) mod runtime;
pub(crate) mod store;

pub use drain_cli::run as drain_cli;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Lost,
    Unavailable,
    Corrupt,
    Capacity,
    Expired,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gateway replica {self:?}")
    }
}
impl std::error::Error for Error {}

impl From<crate::records::RecordError> for Error {
    fn from(error: crate::records::RecordError) -> Self {
        match error {
            crate::records::RecordError::Conflict { .. } => Self::Busy,
            crate::records::RecordError::GuardConflict { .. } => Self::Lost,
            crate::records::RecordError::Malformed(_) => Self::Corrupt,
            _ => Self::Unavailable,
        }
    }
}

impl From<Error> for crate::turns::routed::ExecutionError {
    fn from(error: Error) -> Self {
        match error {
            Error::Busy => Self::Busy,
            Error::Lost => Self::LostLease,
            Error::Expired => Self::Expired,
            Error::Capacity => Self::OperationLimit,
            Error::Corrupt => Self::Corrupt,
            Error::Unavailable => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod partition_tests;

#[cfg(test)]
pub(crate) mod mongo_fixture;
#[cfg(test)]
mod mongo_tests;

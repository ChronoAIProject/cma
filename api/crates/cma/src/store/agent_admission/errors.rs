//! Error conversion at the admission transaction boundary.
use super::*;

impl From<LoadError> for ApplyError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::Domain(error) => Self::Domain(error),
            LoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<super::super::agent_conversation_store::ApplyError> for ApplyError {
    fn from(error: super::super::agent_conversation_store::ApplyError) -> Self {
        use super::super::agent_conversation_store::ApplyError as Budget;
        match error {
            Budget::Domain(error) => Self::Domain(error),
            Budget::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

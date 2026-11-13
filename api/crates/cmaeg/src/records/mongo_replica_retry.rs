//! Retry only a Mongo transaction known to have aborted, never an uncertain commit.
use super::{failure, RecordError};

pub(super) enum AttemptError {
    Aborted,
    Record(RecordError),
}

impl From<RecordError> for AttemptError {
    fn from(error: RecordError) -> Self {
        Self::Record(error)
    }
}

impl From<mongodb::error::Error> for AttemptError {
    fn from(error: mongodb::error::Error) -> Self {
        if error.contains_label("UnknownTransactionCommitResult") {
            return Self::Record(failure(error));
        }
        if error.contains_label("TransientTransactionError") {
            Self::Aborted
        } else {
            Self::Record(failure(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::error::{ErrorKind, WriteConcernError, WriteFailure};

    fn error(labels: &[&str]) -> mongodb::error::Error {
        let concern: WriteConcernError = serde_json::from_value(serde_json::json!({
            "code": 91,
            "codeName": "ShutdownInProgress",
            "errorLabels": labels,
        }))
        .unwrap();
        ErrorKind::Write(WriteFailure::WriteConcernError(concern)).into()
    }

    #[test]
    fn known_aborted_transaction_can_retry() {
        assert!(matches!(
            AttemptError::from(error(&["TransientTransactionError"])),
            AttemptError::Aborted
        ));
    }

    #[test]
    fn ambiguous_commit_takes_precedence_over_aborted_label() {
        for labels in [
            vec!["UnknownTransactionCommitResult"],
            vec![
                "TransientTransactionError",
                "UnknownTransactionCommitResult",
            ],
        ] {
            assert!(matches!(
                AttemptError::from(error(&labels)),
                AttemptError::Record(RecordError::Unavailable(_))
            ));
        }
    }

    #[test]
    fn unlabelled_error_must_not_retry() {
        assert!(matches!(
            AttemptError::from(error(&[])),
            AttemptError::Record(RecordError::Unavailable(_))
        ));
    }
}

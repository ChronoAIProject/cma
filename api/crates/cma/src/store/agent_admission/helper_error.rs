//! Preserve driver evidence until the Agent admission transaction owns retry.
use super::ApplyError;
use layer2_core::sandbox::SandboxStoreError;
use mongodb::error::{Error, UNKNOWN_TRANSACTION_COMMIT_RESULT};

pub(in crate::store) enum HelperError {
    Domain(SandboxStoreError),
    Driver(Error),
}

impl HelperError {
    /// Compatibility callers retain their original store error classification.
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
            // A transient label never overrides uncertainty. Helpers cannot
            // commit, and must not authorize replay of an uncertain outcome.
            HelperError::Driver(error)
                if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) =>
            {
                Self::Domain(SandboxStoreError::Unavailable(error.to_string()))
            }
            HelperError::Driver(error) => Self::Driver(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;
    use mongodb::error::{ErrorKind, WriteConcernError, WriteFailure, TRANSIENT_TRANSACTION_ERROR};

    fn labelled_error(labels: &[&str]) -> Error {
        let error: WriteConcernError = mongodb::bson::deserialize_from_document(doc! {
            "code": 112, "codeName": "WriteConflict", "errmsg": "admission fixture",
            "errorLabels": labels.to_vec(),
        })
        .unwrap();
        ErrorKind::Write(WriteFailure::WriteConcernError(error)).into()
    }

    #[test]
    fn admission_helpers_keep_known_abort_evidence_for_the_transaction_owner() {
        let error = HelperError::Driver(labelled_error(&[TRANSIENT_TRANSACTION_ERROR]));
        let ApplyError::Driver(error) = ApplyError::from(error) else {
            panic!("known abort lost driver evidence");
        };
        assert!(crate::store::sandbox_mongo::is_transient(&error));
    }

    #[test]
    fn admission_helpers_never_retry_uncertain_or_stale_identity_failures() {
        for labels in [
            vec![UNKNOWN_TRANSACTION_COMMIT_RESULT],
            vec![
                TRANSIENT_TRANSACTION_ERROR,
                UNKNOWN_TRANSACTION_COMMIT_RESULT,
            ],
        ] {
            assert!(matches!(
                ApplyError::from(HelperError::Driver(labelled_error(&labels))),
                ApplyError::Domain(SandboxStoreError::Unavailable(_))
            ));
        }
        assert!(matches!(
            ApplyError::from(HelperError::Domain(SandboxStoreError::Conflict)),
            ApplyError::Domain(SandboxStoreError::Conflict)
        ));
        let ApplyError::Driver(error) = ApplyError::from(HelperError::Driver(labelled_error(&[])))
        else {
            panic!("unlabelled driver failure lost classification");
        };
        assert!(!crate::store::sandbox_mongo::is_transient(&error));
    }
}

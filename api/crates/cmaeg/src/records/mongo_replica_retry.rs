//! Retry only a Mongo transaction known to have aborted, never an uncertain commit.
use super::{
    failure,
    replica_observation::{Observation, Phase},
    RecordError,
};
use std::{future::Future, time::Duration};

const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// One deadline includes every attempt and backoff, including direct root writes
/// which do not pass through the partition store's separate outer timeout.
pub(super) async fn run<F: Future<Output = Result<Vec<u64>, AttemptError>>>(
    observation: &Observation,
    mut attempt: impl FnMut() -> F,
) -> Result<Vec<u64>, RecordError> {
    let deadline = tokio::time::Instant::now() + WRITE_TIMEOUT;
    tokio::time::timeout_at(deadline, async {
        for number in 0..5 {
            if tokio::time::Instant::now() >= deadline {
                return Err(deadline_error(observation));
            }
            observation.attempt(number + 1);
            if number > 0 {
                metrics::counter!("cmaeg_replica_transaction_retries_total").increment(1);
            }
            match attempt().await {
                Ok(versions) => return Ok(versions),
                Err(AttemptError::Record(error)) => return Err(error),
                Err(AttemptError::Aborted) => {
                    if number == 4 {
                        break;
                    }
                    observation.phase(Phase::Backoff);
                    tokio::time::sleep(Duration::from_millis(5 << number)).await;
                }
            }
        }
        Err(RecordError::Unavailable(
            "replica transaction contention budget exhausted".into(),
        ))
    })
    .await
    .map_err(|_| deadline_error(observation))?
}

fn deadline_error(observation: &Observation) -> RecordError {
    // Cancellation may race a server commit. Preserve uncertainty: there is
    // no retry here and the caller must reconcile the same retained intent.
    observation.deadline();
    RecordError::Unavailable("replica transaction deadline exceeded".into())
}

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

    pub(super) fn error(labels: &[&str]) -> mongodb::error::Error {
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

#[cfg(test)]
#[path = "mongo_replica_deadline_tests.rs"]
pub(super) mod deadline_tests;

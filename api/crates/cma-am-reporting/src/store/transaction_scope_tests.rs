use super::*;

async fn finish<T>(session: &mut impl Transaction, result: Result<T>) -> Result<T> {
    super::finish(session, result, |_| Box::pin(async {})).await
}

use mongodb::{
    bson::doc,
    error::{WriteConcernError, WriteFailure},
};
use std::{collections::VecDeque, sync::Arc};

fn labelled(labels: &[&str]) -> Error {
    let error: WriteConcernError = mongodb::bson::deserialize_from_document(doc! {
        "code":112, "codeName":"WriteConflict", "errmsg":"transaction test",
        "errorLabels":labels.to_vec(),
    })
    .unwrap();
    mongodb::error::ErrorKind::Write(WriteFailure::WriteConcernError(error)).into()
}

struct Script {
    commits: VecDeque<mongodb::error::Result<()>>,
    commit_calls: usize,
    abort_calls: usize,
}
#[async_trait::async_trait]
impl Transaction for Script {
    async fn commit(&mut self) -> mongodb::error::Result<()> {
        self.commit_calls += 1;
        self.commits.pop_front().expect("unexpected commit")
    }
    async fn abort(&mut self) -> mongodb::error::Result<()> {
        self.abort_calls += 1;
        Err(Error::custom("injected abort failure"))
    }
}
fn script(commits: Vec<mongodb::error::Result<()>>) -> Script {
    Script {
        commits: commits.into(),
        commit_calls: 0,
        abort_calls: 0,
    }
}

#[tokio::test]
async fn body_error_aborts_once_and_abort_failure_preserves_domain_and_driver_errors() {
    let mut session = script(vec![]);
    let error = finish::<()>(&mut session, Err(domain(MetricsError::Busy)))
        .await
        .unwrap_err();
    assert!(matches!(error, ErrorKind::Domain(MetricsError::Busy)));
    assert!(!error.retryable());
    assert_eq!(session.abort_calls, 1);
    assert_eq!(session.commit_calls, 0);
    let mut session = script(vec![]);
    let error = finish::<()>(&mut session, Err(mongo(Error::custom(123_u32))))
        .await
        .unwrap_err();
    let ErrorKind::Mongo(error) = error else {
        panic!("original driver error lost")
    };
    assert_eq!(error.get_custom::<u32>(), Some(&123));
    assert_eq!(session.abort_calls, 1);
    assert_eq!(session.commit_calls, 0);
}

#[tokio::test]
async fn definite_and_uncertain_commit_failures_never_abort_or_replace_the_original_error() {
    for uncertain in [false, true] {
        let original = Arc::new(());
        let mut commits = Vec::new();
        if uncertain {
            commits.push(Err(labelled(&[UNKNOWN_TRANSACTION_COMMIT_RESULT])));
        }
        commits.push(Err(Error::custom(Arc::clone(&original))));
        let mut session = script(commits);
        let error = finish(&mut session, Ok(())).await.unwrap_err();
        let error = match (uncertain, error) {
            (false, ErrorKind::Mongo(error)) | (true, ErrorKind::Uncertain(error)) => error,
            (_, error) => panic!("commit failure classification changed: {error:?}"),
        };
        assert!(Arc::ptr_eq(
            error
                .get_custom::<Arc<()>>()
                .expect("original commit error"),
            &original,
        ));
        assert_eq!(session.abort_calls, 0);
        assert_eq!(session.commit_calls, if uncertain { 2 } else { 1 });
    }
}

#[tokio::test]
async fn uncertain_commit_retries_commit_only_and_keeps_uncertainty() {
    let mut session = script(vec![
        Err(labelled(&[UNKNOWN_TRANSACTION_COMMIT_RESULT])),
        Ok(()),
    ]);
    assert_eq!(finish(&mut session, Ok(42)).await.unwrap(), 42);
    assert_eq!(session.commit_calls, 2);
    assert_eq!(session.abort_calls, 0);
    for later in [
        labelled(&[TRANSIENT_TRANSACTION_ERROR]),
        Error::custom("later failure"),
    ] {
        let mut session = script(vec![
            Err(labelled(&[UNKNOWN_TRANSACTION_COMMIT_RESULT])),
            Err(later),
        ]);
        let error = finish(&mut session, Ok(42)).await.unwrap_err();
        assert!(matches!(error, ErrorKind::Uncertain(_)));
        assert!(!error.retryable());
        assert_eq!(session.commit_calls, 2);
        assert_eq!(session.abort_calls, 0);
    }
}

#[tokio::test]
async fn commit_exhaustion_is_bounded_and_only_known_abort_can_restart() {
    let mut session = script(
        (0..COMMIT_ATTEMPTS)
            .map(|_| Err(labelled(&[UNKNOWN_TRANSACTION_COMMIT_RESULT])))
            .collect(),
    );
    let error = finish(&mut session, Ok(())).await.unwrap_err();
    assert!(!error.retryable());
    assert_eq!(session.commit_calls, COMMIT_ATTEMPTS);
    assert_eq!(session.abort_calls, 0);
    let mut session = script(vec![Err(labelled(&[TRANSIENT_TRANSACTION_ERROR]))]);
    let error = finish(&mut session, Ok(())).await.unwrap_err();
    assert!(error.retryable());
    assert_eq!(session.abort_calls, 0);
}

#[test]
fn raw_command_labels_follow_the_same_retry_rules() {
    for (labels, retryable) in [
        (vec![], false),
        (vec![TRANSIENT_TRANSACTION_ERROR], true),
        (vec![UNKNOWN_TRANSACTION_COMMIT_RESULT], false),
        (
            vec![
                TRANSIENT_TRANSACTION_ERROR,
                UNKNOWN_TRANSACTION_COMMIT_RESULT,
            ],
            false,
        ),
    ] {
        assert_eq!(
            ErrorKind::Command(doc! {"errorLabels":&labels}).retryable(),
            retryable
        );
        assert_eq!(mongo(labelled(&labels)).retryable(), retryable);
    }
}

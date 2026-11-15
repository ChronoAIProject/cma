use super::*;
use mongodb::error::{
    Error, ErrorKind, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT,
    WriteConcernError, WriteFailure,
};

fn labelled_error(labels: &[&str]) -> Error {
    // The driver's public conversion retains labels supplied by a write-concern
    // response. Exercise the real Error label API, without matching its text.
    let error: WriteConcernError = mongodb::bson::deserialize_from_document(doc! {
        "code": 112, "codeName": "WriteConflict", "errmsg": "synthetic conflict",
        "errorLabels": labels.to_vec(),
    })
    .unwrap();
    ErrorKind::Write(WriteFailure::WriteConcernError(error)).into()
}

#[test]
fn ownership_fence_known_abort_is_a_retryable_store_conflict() {
    let error = labelled_error(&[TRANSIENT_TRANSACTION_ERROR]);
    assert!(error.contains_label(TRANSIENT_TRANSACTION_ERROR));
    assert_eq!(fence_write_error(error), SandboxStoreError::Conflict);
}

#[test]
fn ownership_fence_unknown_outcomes_and_unlabelled_errors_stay_unavailable() {
    for labels in [
        vec![],
        vec![UNKNOWN_TRANSACTION_COMMIT_RESULT],
        vec![
            TRANSIENT_TRANSACTION_ERROR,
            UNKNOWN_TRANSACTION_COMMIT_RESULT,
        ],
    ] {
        assert!(matches!(
            fence_write_error(labelled_error(&labels)),
            SandboxStoreError::Unavailable(_)
        ));
    }
    assert!(matches!(
        fence_write_error(Error::custom("unavailable")),
        SandboxStoreError::Unavailable(_)
    ));
}

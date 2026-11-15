use super::*;
use mongodb::bson::doc;

#[test]
fn inputs_are_bounded_and_reserved_ids_are_rejected() {
    assert!(identifier(&"ab".repeat(32)));
    assert!(identifier(&"A".repeat(43)));
    for invalid in [
        "quota".to_owned(),
        "".to_owned(),
        "!".repeat(43),
        "G".repeat(64),
    ] {
        assert!(!identifier(&invalid));
    }
    assert!(!payload_valid(&[]));
    assert!(payload_valid(&vec![1; MAX_PAYLOAD]));
    assert!(!payload_valid(&vec![1; MAX_PAYLOAD + 1]));
}

#[test]
fn malformed_rows_fail_closed() {
    assert!(matches!(mongo::stored(doc! {}), Err(StoreError::Invalid)));
    let mut row = doc! {"subject_hash":"ab".repeat(32),"payload":mongo::binary(vec![1]),"observed_at":mongodb::bson::DateTime::now(),
    "created_at":mongodb::bson::DateTime::now(),"last_seen":mongodb::bson::DateTime::now(),
    "absolute_at":mongodb::bson::DateTime::now(),"access_expires_at":mongodb::bson::DateTime::now()};
    assert!(mongo::stored(row.clone()).is_ok());
    row.insert("payload", mongo::binary(vec![]));
    assert!(matches!(mongo::stored(row), Err(StoreError::Invalid)));
}

#[test]
fn uncertain_commit_never_becomes_a_known_abort() {
    use mongodb::error::{ErrorKind, WriteConcernError, WriteFailure};
    for (labels, retry) in [
        (vec!["TransientTransactionError"], true),
        (vec!["UnknownTransactionCommitResult"], false),
        (
            vec![
                "TransientTransactionError",
                "UnknownTransactionCommitResult",
            ],
            false,
        ),
        (vec![], false),
    ] {
        let error: WriteConcernError = serde_json::from_value(
            serde_json::json!({"code":91,"codeName":"ShutdownInProgress","errorLabels":labels}),
        )
        .unwrap();
        let error = ErrorKind::Write(WriteFailure::WriteConcernError(error)).into();
        assert_eq!(create::known_abort(&error), retry);
    }
}

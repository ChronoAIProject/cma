use super::*;
use mongodb::error::{Error, ErrorKind, InsertManyError};

fn batch_error(errors: serde_json::Value, concern: serde_json::Value) -> Error {
    let failure: InsertManyError = serde_json::from_value(serde_json::json!({
        "writeErrors": errors, "writeConcernError": concern,
    }))
    .unwrap();
    ErrorKind::InsertMany(failure).into()
}

#[test]
fn only_duplicate_only_batches_without_write_concern_errors_are_successful() {
    let duplicate = serde_json::json!({"index":0,"code":11000,"errmsg":"duplicate"});
    let invalid = serde_json::json!({"index":1,"code":121,"errmsg":"validation"});
    assert!(duplicate_batch(&batch_error(
        serde_json::json!([duplicate]),
        serde_json::Value::Null
    )));
    for errors in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!([duplicate, invalid]),
    ] {
        assert!(!duplicate_batch(&batch_error(
            errors,
            serde_json::Value::Null
        )));
    }
    assert!(!duplicate_batch(&batch_error(
        serde_json::json!([duplicate]),
        serde_json::json!({"code":64,"codeName":"WriteConcernFailed","errmsg":"timeout"})
    )));
    assert!(!duplicate_batch(&Error::from(std::io::Error::other(
        "network"
    ))));
}

use super::*;
use mongodb::bson::Bson;

fn parent<'a>(row: &'a mut Document, path: &[&str]) -> &'a mut Document {
    let mut current = row;
    for part in path {
        current = current.get_document_mut(*part).unwrap();
    }
    current
}

#[test]
fn capacity_recovery_decode_refuses_missing_required_nullables() {
    for (path, key) in [
        (vec!["work"], "lease"),
        (vec!["work"], "completed_at"),
        (vec!["work", "target"], "active_turn"),
    ] {
        let mut row = encode(&test_support::stored()).unwrap();
        assert_eq!(parent(&mut row, &path).remove(key), Some(Bson::Null));
        assert!(decode(row).is_err(), "missing {path:?}.{key}");
    }
}

#[test]
fn capacity_recovery_decode_refuses_numeric_bson_normalization() {
    for (path, key, wrong) in [
        (vec!["work"], "revision", Bson::Int32(1)),
        (vec!["work"], "format_version", Bson::Int64(1)),
        (vec!["work"], "attempts", Bson::Int32(0)),
        (vec!["work"], "next_fence", Bson::Int32(3)),
        (vec!["work", "request"], "failure_seq", Bson::Int32(2)),
        (
            vec!["work", "target"],
            "conversation_generation",
            Bson::Int32(1),
        ),
        (vec!["work", "target"], "active_turn", Bson::Int32(1)),
        (vec!["result"], "agent_revision", Bson::Int32(2)),
        (vec!["result"], "through_seq", Bson::Int32(2)),
    ] {
        let mut row = encode(&test_support::stored()).unwrap();
        parent(&mut row, &path).insert(key, wrong);
        assert!(decode(row).is_err(), "normalized {path:?}.{key}");
    }
}

#[test]
fn capacity_recovery_decode_preserves_runtime_omission_and_rejects_null() {
    for key in ["boot_id", "provision_generation"] {
        let mut row = encode(&test_support::stored()).unwrap();
        let path = ["work", "target", "runtime"];
        parent(&mut row, &path).remove(key);
        assert!(decode(row.clone()).is_ok(), "allowed omission {key}");
        parent(&mut row, &path).insert(key, Bson::Null);
        assert!(decode(row).is_err(), "explicit null {key}");
    }
}

#[test]
fn capacity_recovery_decode_refuses_nested_extensions_and_oversize_fields() {
    for (path, key, wrong) in [
        (
            vec!["work", "target", "runtime"],
            "extra",
            Bson::Boolean(true),
        ),
        (
            vec!["work", "target", "runtime"],
            "boot_id",
            Bson::String("a".repeat(33)),
        ),
        (
            vec!["work", "target", "runtime"],
            "runtime_version",
            Bson::String("x".repeat(16_385)),
        ),
        (
            vec!["work", "target", "runtime"],
            "provision_generation",
            Bson::Int32(1),
        ),
        (
            vec!["work", "target", "original_open"],
            "step",
            Bson::String("publish_ready".into()),
        ),
    ] {
        let mut row = encode(&test_support::stored()).unwrap();
        parent(&mut row, &path).insert(key, wrong);
        assert!(decode(row).is_err(), "invalid {path:?}.{key}");
    }
}

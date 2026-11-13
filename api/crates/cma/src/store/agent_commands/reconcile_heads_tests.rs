use super::*;
use crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;

fn row() -> Document {
    doc! { "_id": "agt_hint", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
    "status": "working", "record": { "conversation": {
        "last_seq": 18_i64, "runtime_generation": 2_i64, "active_turn": 7_i64
    } } }
}

#[test]
fn projection_accepts_known_statuses_and_explicit_null_optional_fields() {
    for (status, live) in [("working", true), ("paused", true), ("stopped", false)] {
        let mut document = row();
        document.insert("status", status);
        let head = decode(&document).unwrap();
        assert_eq!(head.last_seq, 18);
        assert_eq!(head.runtime_generation, Some(2));
        assert_eq!(head.active_turn, Some(7));
        assert_eq!(head.live, live);
    }
    let mut document = row();
    let conversation = document
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("conversation")
        .unwrap();
    conversation.insert("runtime_generation", Bson::Null);
    conversation.insert("active_turn", Bson::Null);
    let head = decode(&document).unwrap();
    assert_eq!(head.runtime_generation, None);
    assert_eq!(head.active_turn, None);
}

#[test]
fn missing_wrong_typed_negative_or_unknown_fields_cannot_skip_reconciliation() {
    for field in ["_id", "status", "document_schema_version", "record"] {
        let mut document = row();
        document.remove(field);
        assert!(decode(&document).is_none(), "missing {field}");
        let mut document = row();
        document.insert(field, false);
        assert!(decode(&document).is_none(), "mistyped {field}");
    }
    for field in ["last_seq", "runtime_generation", "active_turn"] {
        for value in [
            None,
            Some(Bson::String("2".into())),
            Some(Bson::Int64(-1)),
            Some(Bson::Double(2.0)),
        ] {
            let mut document = row();
            let conversation = document
                .get_document_mut("record")
                .unwrap()
                .get_document_mut("conversation")
                .unwrap();
            conversation.remove(field);
            if let Some(value) = value {
                conversation.insert(field, value);
            }
            assert!(decode(&document).is_none(), "invalid {field}");
        }
    }
    let mut document = row();
    document.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION + 1);
    assert!(decode(&document).is_none());
    let mut document = row();
    document.insert("status", "future_status");
    assert!(decode(&document).is_none());
}

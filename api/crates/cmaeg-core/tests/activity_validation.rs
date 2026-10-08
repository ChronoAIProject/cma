use cmaeg_core::{
    activity::Activity,
    channel::{PartitionKey, PartitionRule},
};
use serde_json::{json, Value};

fn activity() -> Value {
    serde_json::from_str(include_str!("fixtures/activity.json")).unwrap()
}

fn validate(value: Value) -> Result<(), cmaeg_core::ValidationError> {
    serde_json::from_value::<Activity>(value)
        .unwrap()
        .validate()
}

#[test]
fn rejects_unsupported_activity_schema() {
    let mut value = activity();
    value["schema_version"] = json!(2);
    assert_eq!(validate(value).unwrap_err().pointer, "/schema_version");
}

#[test]
fn activity_identity_limits_distinguish_relay_and_custom_sources() {
    for (source, max) in [
        (activity()["source"].clone(), 512),
        (json!({"type":"custom"}), 4096),
    ] {
        for pointer in ["/actor/id", "/conversation/id"] {
            let mut value = activity();
            value["source"] = source.clone();
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(max));
            validate(value.clone()).unwrap();
            for invalid in [String::new(), "x".repeat(max + 1)] {
                *value.pointer_mut(pointer).unwrap() = json!(invalid);
                assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
            }
        }
    }
}

#[test]
fn rejects_activity_time_and_thread_string_overflow() {
    for (pointer, max) in [("/occurred_at", 128), ("/conversation/thread_id", 512)] {
        let mut value = activity();
        value["conversation"]["thread_id"] = json!("thread");
        for size in [0, max + 1] {
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
        }
        *value.pointer_mut(pointer).unwrap() = json!("x".repeat(max));
        validate(value).unwrap();
    }
}

#[test]
fn message_activity_accepts_full_context_and_rejects_overflow() {
    let mut value = activity();
    value["kind"]["text"] = json!("é".repeat(16 * 1_024));
    let attachment = json!({"reference":"r".repeat(4096),"kind":"file","filename":"notes.txt","mime_type":"text/plain","size_bytes":42});
    value["kind"]["attachments"] = json!(vec![attachment.clone(); 16]);
    validate(value.clone()).unwrap();
    value["kind"]["text"] = json!("x".repeat(32 * 1_024 + 1));
    assert_eq!(validate(value.clone()).unwrap_err().pointer, "/kind/text");
    value["kind"]["text"] = json!("");
    value["kind"]["attachments"] = json!(vec![attachment; 17]);
    assert_eq!(
        validate(value.clone()).unwrap_err().pointer,
        "/kind/attachments"
    );
    value["kind"]["attachments"].as_array_mut().unwrap().pop();
    for reference in [String::new(), "x".repeat(4097)] {
        value["kind"]["attachments"][0]["reference"] = json!(reference);
        assert_eq!(
            validate(value.clone()).unwrap_err().pointer,
            "/kind/attachments/0/reference"
        );
    }
}

#[test]
fn interaction_and_command_activities_validate_their_own_fields() {
    for (kind, fields) in [
        (
            json!({"type":"interaction","block_id":"b","action_id":"a","values":{"answer":42}}),
            vec![("block_id", 1, 512), ("action_id", 1, 512)],
        ),
        (
            json!({"type":"command","name":"run","args":""}),
            vec![("name", 1, 512), ("args", 0, 32 * 1024)],
        ),
    ] {
        for (field, min, max) in fields {
            let mut value = activity();
            value["kind"] = kind.clone();
            value["kind"][field] = json!("x".repeat(max));
            validate(value.clone()).unwrap();
            value["kind"][field] = json!("x".repeat(max + 1));
            assert_eq!(
                validate(value.clone()).unwrap_err().pointer,
                format!("/kind/{field}")
            );
            if min > 0 {
                value["kind"][field] = json!("");
                assert_eq!(
                    validate(value).unwrap_err().pointer,
                    format!("/kind/{field}")
                );
            }
        }
    }
}

#[test]
fn event_partition_uses_subject_then_source_and_preserves_thread_identity() {
    let mut value = activity();
    let ce: Value = serde_json::from_str(include_str!("fixtures/cloudevent.json")).unwrap();
    value["source"] = json!({"type":"custom"});
    value["kind"] = json!({"type":"event","ce":ce});
    let rule = PartitionRule {
        version: 1,
        key: PartitionKey::EventSubject,
        custom_key: None,
    };
    let parsed: Activity = serde_json::from_value(value.clone()).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.partition_key(&rule), "urn:test:source");
    value["kind"]["ce"]["subject"] = json!("subject");
    let parsed: Activity = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(parsed.partition_key(&rule), "subject");
    value["kind"]["ce"]["specversion"] = json!("0.3");
    assert_eq!(validate(value).unwrap_err().pointer, "/kind/ce/specversion");
    let mut value = activity();
    value["conversation"]["thread_id"] = json!("thread-1");
    let parsed: Activity = serde_json::from_value(value).unwrap();
    assert_eq!(
        parsed.partition_key(&PartitionRule {
            version: 1,
            key: PartitionKey::Thread,
            custom_key: None,
        }),
        "chat-1\u{1f}thread-1"
    );
}

#[test]
fn message_album_identity_is_bounded_name_text() {
    let mut value = activity();
    validate(value.clone()).unwrap();
    for album_id in [
        "13579246801357924".to_owned(),
        "x".repeat(128),
        "相册".into(),
    ] {
        value["kind"]["album_id"] = json!(album_id);
        validate(value.clone()).unwrap();
    }
    for album_id in [
        String::new(),
        "x".repeat(129),
        "album\u{202E}".into(),
        "album\u{0007}".into(),
    ] {
        value["kind"]["album_id"] = json!(album_id);
        assert_eq!(
            validate(value.clone()).unwrap_err().pointer,
            "/kind/album_id"
        );
    }
}

#[test]
fn album_identity_round_trips_only_on_messages() {
    let mut value = activity();
    let plain: Activity = serde_json::from_value(value.clone()).unwrap();
    assert!(!serde_json::to_string(&plain).unwrap().contains("album_id"));
    value["kind"]["album_id"] = json!("13579246801357924");
    let grouped: Activity = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&grouped).unwrap(), value);
    value["kind"] = json!({"type":"command","name":"run","args":"","album_id":"1"});
    assert!(serde_json::from_value::<Activity>(value).is_err());
}

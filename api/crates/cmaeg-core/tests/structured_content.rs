use cmaeg_core::message::{OutboundMessage, StructuredContent};
use serde_json::{json, Value};

fn samples() -> Vec<Value> {
    vec![
        json!({"type":"contact","phone_number":"+6512345678","first_name":"Alice","last_name":"Example","vcard":"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Alice\r\nEND:VCARD"}),
        json!({"type":"location","latitude":-90.0,"longitude":180.0,"horizontal_accuracy":1500.0}),
        json!({"type":"venue","latitude":1.3,"longitude":103.8,"title":"Library","address":"Main Street"}),
    ]
}
#[test]
fn structured_content_is_exclusive_and_legacy_bytes_are_unchanged() {
    let legacy: OutboundMessage = serde_json::from_value(json!({"text":"legacy"})).unwrap();
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"text":"legacy","blocks":[]}"#
    );
    for content in samples() {
        let wire = json!({"structured_content":content,"blocks":[]});
        let message: OutboundMessage = serde_json::from_value(wire.clone()).unwrap();
        message.validate().unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), wire);
        for (key, value) in [
            ("text", json!("other")),
            ("blocks", json!([{"type":"text","text":"other"}])),
            ("thread_reply", json!(true)),
            ("reply_with_received_sticker", json!(true)),
            ("clear_native_attachments", json!(true)),
            ("native_media_update", json!({"caption":"other"})),
            (
                "native_attachments",
                json!([{"kind":"file","source":{"type":"base64","data":"AQ=="}}]),
            ),
        ] {
            let mut mixed = wire.clone();
            mixed[key] = value;
            assert!(
                serde_json::from_value::<OutboundMessage>(mixed)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{key}"
            );
        }
    }
    assert!(serde_json::from_value::<OutboundMessage>(json!({"structured_content":null})).is_err());
}
#[test]
fn static_content_refuses_live_tracking_recipients_nonfinite_and_out_of_range_values() {
    for content in samples() {
        for key in [
            "chat_id",
            "recipient",
            "url",
            "live_period",
            "heading",
            "proximity_alert_radius",
        ] {
            let mut bad = content.clone();
            bad[key] = json!(1);
            assert!(
                serde_json::from_value::<StructuredContent>(bad).is_err(),
                "{key}"
            );
        }
    }
    for (latitude, longitude, horizontal_accuracy) in [
        (91.0, 0.0, None),
        (0.0, -181.0, None),
        (f64::NAN, 0.0, None),
        (0.0, f64::INFINITY, None),
        (0.0, 0.0, Some(-1.0)),
        (0.0, 0.0, Some(1500.1)),
    ] {
        assert!(StructuredContent::Location {
            latitude,
            longitude,
            horizontal_accuracy
        }
        .validate()
        .is_err());
    }
    for (field, value) in [
        ("phone_number", " ".into()),
        ("first_name", "\0".into()),
        ("last_name", "x".repeat(1025)),
        ("vcard", "x".repeat(2049)),
        ("vcard", "BEGIN:\u{202e}VCARD".into()),
    ] {
        let mut bad = samples().remove(0);
        bad[field] = Value::String(value);
        assert!(
            serde_json::from_value::<StructuredContent>(bad)
                .unwrap()
                .validate()
                .is_err(),
            "{field}"
        );
    }
}

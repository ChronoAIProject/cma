//! The attachment domain: every fact bound and every locator/source pairing.
use cmaeg_core::{
    activity::{validate_attachments, Activity, Attachment, AttachmentSource, SourceKind},
    channel::Platform,
    ValidationError,
};
use serde_json::{json, Value};

const BASE: &str = "/kind/attachments";

fn relay() -> SourceKind {
    SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "route-1".into(),
    }
}

fn telegram() -> SourceKind {
    SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "123456".into(),
    }
}

fn sources() -> [SourceKind; 3] {
    [relay(), telegram(), SourceKind::Custom]
}

fn item(facts: Value) -> Attachment {
    let mut value = json!({"reference":"fixture:item-0","kind":"file"});
    value
        .as_object_mut()
        .unwrap()
        .extend(facts.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn check(source: &SourceKind, facts: Value) -> Result<(), ValidationError> {
    validate_attachments(&[item(facts)], source, BASE)
}

fn refused_at(source: &SourceKind, facts: Value, field: &str) {
    let error =
        check(source, facts.clone()).expect_err(&format!("{facts} must be refused for {source:?}"));
    assert_eq!(error.pointer, format!("{BASE}/0/{field}"), "{facts}");
}

fn telegram_locator() -> Value {
    json!({"type":"telegram","file_id":"AgACAgIAAxkBAAIB","file_unique_id":"AQADBAADr6cxGw"})
}

/// One valid locator per source, in the order of [`sources`].
fn native_locators() -> [Value; 3] {
    [
        json!({"type":"nyxid_relay","index":0}),
        telegram_locator(),
        json!({"type":"url","url":"https://files.example/a.png"}),
    ]
}

#[test]
fn facts_at_their_bounds_validate_for_every_source() {
    let facts = json!({
        "filename": format!("{}.txt", "é".repeat(125)),
        "mime_type": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "size_bytes": u64::MAX,
        "variant": "video_note",
        "duration_seconds": 86_400,
        "width": 1,
        "height": 100_000,
        "emoji": "👍".repeat(8),
    });
    assert_eq!(facts["filename"].as_str().unwrap().len(), 254);
    for source in sources() {
        check(&source, facts.clone()).unwrap();
        check(&source, json!({})).unwrap();
        check(
            &source,
            json!({"filename":"x".repeat(255),"emoji":"x".repeat(32)}),
        )
        .unwrap();
    }
    validate_attachments(&[], &relay(), BASE).unwrap();
}

#[test]
fn facts_outside_their_bounds_are_refused_at_their_pointer() {
    for source in sources() {
        for (facts, field) in [
            (json!({"filename":""}), "filename"),
            (json!({"filename":"x".repeat(256)}), "filename"),
            (json!({"filename":"invoice\u{202E}fdp.exe"}), "filename"),
            (json!({"filename":"bell\u{0007}.txt"}), "filename"),
            (json!({"filename":"next\u{0085}line"}), "filename"),
            (json!({"mime_type":"Image/JPEG"}), "mime_type"),
            (
                json!({"mime_type":"text/plain; charset=utf-8"}),
                "mime_type",
            ),
            (json!({"mime_type":"text/plain;charset=utf-8"}), "mime_type"),
            (json!({"mime_type":"not a mime"}), "mime_type"),
            (json!({"mime_type":""}), "mime_type"),
            (json!({"duration_seconds":86_401}), "duration_seconds"),
            (json!({"width":640}), "width"),
            (json!({"height":480}), "height"),
            (json!({"width":0,"height":480}), "width"),
            (json!({"width":640,"height":0}), "height"),
            (json!({"width":100_001,"height":480}), "width"),
            (json!({"width":640,"height":100_001}), "height"),
            (json!({"emoji":""}), "emoji"),
            (json!({"emoji":"x".repeat(33)}), "emoji"),
            (json!({"emoji":"\u{2066}👍"}), "emoji"),
        ] {
            refused_at(&source, facts, field);
        }
    }
}

#[test]
fn each_locator_type_is_accepted_only_with_its_own_source() {
    let custom_inline = json!({"type":"inline","pointer":"/attachments/0/url"});
    for (position, source) in sources().iter().enumerate() {
        for (candidate, locator) in native_locators().into_iter().enumerate() {
            let result = check(source, json!({ "source": locator }));
            if candidate == position {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().pointer, format!("{BASE}/0/source"));
            }
        }
        let inline = check(source, json!({ "source": custom_inline }));
        if matches!(source, SourceKind::Custom) {
            inline.unwrap();
        } else {
            assert_eq!(inline.unwrap_err().pointer, format!("{BASE}/0/source"));
        }
    }
}

#[test]
fn locators_at_their_bounds_validate() {
    let file_id = "A".repeat(256);
    let file_unique_id = "-".repeat(128);
    let url = format!("https://files.example/{}", "a".repeat(2_048 - 22));
    let pointer = format!("/{}", "a".repeat(255));
    for (source, locator) in [
        (relay(), json!({"type":"nyxid_relay","index":31})),
        (
            telegram(),
            json!({"type":"telegram","file_id":file_id,"file_unique_id":file_unique_id}),
        ),
        (SourceKind::Custom, json!({"type":"url","url":url})),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://[2001:db8::1]:8443/a?b=c"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"inline","pointer":pointer}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"inline","pointer":"/a~0b/~1c/"}),
        ),
        (SourceKind::Custom, json!({"type":"inline","pointer":"/"})),
    ] {
        check(&source, json!({ "source": locator })).unwrap();
    }
}

#[test]
fn malformed_locators_are_refused() {
    let long_url = format!("https://files.example/{}", "a".repeat(2_048 - 21));
    let long_pointer = format!("/{}", "a".repeat(256));
    for (source, locator) in [
        (relay(), json!({"type":"nyxid_relay","index":32})),
        (
            telegram(),
            json!({"type":"telegram","file_id":"AgAC/AgIA","file_unique_id":"AQAD"}),
        ),
        (
            telegram(),
            json!({"type":"telegram","file_id":"","file_unique_id":"AQAD"}),
        ),
        (
            telegram(),
            json!({"type":"telegram","file_id":"AgAC","file_unique_id":"A".repeat(129)}),
        ),
        (
            telegram(),
            json!({"type":"telegram","file_id":"A".repeat(257),"file_unique_id":"AQAD"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"http://files.example/a.png"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"http://localhost/a.png"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://user:pass@files.example/a.png"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://user@files.example/a.png"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://files.example/a.png#part"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://files.example/a b.png"}),
        ),
        (SourceKind::Custom, json!({"type":"url","url":"/a.png"})),
        (SourceKind::Custom, json!({"type":"url","url":long_url})),
        (
            SourceKind::Custom,
            json!({"type":"url","url":"data:image/png;base64,AAAA"}),
        ),
        (
            SourceKind::Custom,
            json!({"type":"inline","pointer":"attachments/0"}),
        ),
        (SourceKind::Custom, json!({"type":"inline","pointer":""})),
        (
            SourceKind::Custom,
            json!({"type":"inline","pointer":"/attachments~2/0"}),
        ),
        (SourceKind::Custom, json!({"type":"inline","pointer":"/a~"})),
        (
            SourceKind::Custom,
            json!({"type":"inline","pointer":long_pointer}),
        ),
    ] {
        refused_at(&source, json!({ "source": locator }), "source");
    }
}

#[test]
fn a_thumbnail_is_only_a_valid_telegram_locator_on_a_telegram_activity() {
    check(&telegram(), json!({ "thumbnail": telegram_locator() })).unwrap();
    for (source, thumbnail) in [
        (
            SourceKind::Custom,
            json!({"type":"url","url":"https://files.example/a.png"}),
        ),
        (relay(), telegram_locator()),
        (SourceKind::Custom, telegram_locator()),
        (telegram(), json!({"type":"nyxid_relay","index":0})),
        (
            telegram(),
            json!({"type":"url","url":"https://files.example/a.png"}),
        ),
        (
            telegram(),
            json!({"type":"telegram","file_id":"a/b","file_unique_id":"AQAD"}),
        ),
    ] {
        refused_at(&source, json!({ "thumbnail": thumbnail }), "thumbnail");
    }
}

#[test]
fn unknown_keys_and_values_are_refused_when_parsing() {
    let mut attachment = json!({"reference":"r","kind":"file"});
    serde_json::from_value::<Attachment>(attachment.clone()).unwrap();
    attachment["transcript"] = json!("derived content");
    assert!(serde_json::from_value::<Attachment>(attachment).is_err());
    for locator in [
        json!({"type":"nyxid_relay","index":0,"message_id":"m"}),
        json!({"type":"telegram","file_id":"a","file_unique_id":"b","file_path":"p"}),
        json!({"type":"url","url":"https://files.example","headers":{}}),
        json!({"type":"lark","key":"k"}),
        json!({"type":"nyxid_relay","index":256}),
        json!({"type":"nyxid_relay","index":-1}),
    ] {
        assert!(
            serde_json::from_value::<AttachmentSource>(locator.clone()).is_err(),
            "{locator}"
        );
    }
    let unknown_variant = json!({"reference":"r","kind":"file","variant":"hologram"});
    assert!(serde_json::from_value::<Attachment>(unknown_variant).is_err());
}

#[test]
fn new_facts_round_trip_and_are_omitted_when_absent() {
    let full = json!({
        "reference":"telegram:AQADBAADr6cxGw","kind":"video","filename":"clip.mp4",
        "mime_type":"video/mp4","size_bytes":1,"source":telegram_locator(),
        "variant":"video","duration_seconds":12,"width":640,"height":480,
        "thumbnail":telegram_locator(),"emoji":"🎬"
    });
    let parsed: Attachment = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), full);
    let legacy = item(json!({}));
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"reference":"fixture:item-0","kind":"file"}"#
    );
}

#[test]
fn activity_validation_reports_attachment_pointers_against_its_own_source() {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/activity.json")).unwrap();
    value["kind"]["attachments"] = json!([
        {"reference":"r0","kind":"image","source":{"type":"nyxid_relay","index":0}},
        {"reference":"r1","kind":"image","source":telegram_locator()},
    ]);
    let activity: Activity = serde_json::from_value(value).unwrap();
    assert_eq!(
        activity.validate().unwrap_err().pointer,
        "/kind/attachments/1/source"
    );
}

fn custom_event(attachments: Value) -> Value {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/activity.json")).unwrap();
    let ce: Value = serde_json::from_str(include_str!("fixtures/cloudevent.json")).unwrap();
    value["source"] = json!({"type":"custom"});
    value["kind"] = json!({"type":"event","ce":ce,"attachments":attachments});
    value
}

#[test]
fn custom_event_attachments_validate_and_round_trip() {
    let value = custom_event(json!([{
        "reference":"order-9:item-0","kind":"image","mime_type":"image/png",
        "source":{"type":"url","url":"https://files.example/a.png"}
    }]));
    let activity: Activity = serde_json::from_value(value.clone()).unwrap();
    activity.validate().unwrap();
    assert_eq!(serde_json::to_value(&activity).unwrap(), value);
    let empty: Activity = serde_json::from_value(custom_event(json!([]))).unwrap();
    empty.validate().unwrap();
    assert!(!serde_json::to_string(&empty)
        .unwrap()
        .contains("attachments"));
}

#[test]
fn custom_event_attachments_are_held_to_the_same_domain() {
    let refused = |attachments: Value| {
        serde_json::from_value::<Activity>(custom_event(attachments))
            .unwrap()
            .validate()
            .unwrap_err()
            .pointer
    };
    assert_eq!(
        refused(json!([{"reference":"r","kind":"file","source":telegram_locator()}])),
        "/kind/attachments/0/source"
    );
    assert_eq!(
        refused(json!([{"reference":"r","kind":"file","mime_type":"Text/Plain"}])),
        "/kind/attachments/0/mime_type"
    );
    assert_eq!(
        refused(json!(vec![json!({"reference":"r","kind":"file"}); 17])),
        "/kind/attachments"
    );
    let mut invalid_event = custom_event(json!([{"reference":"r","kind":"file"}]));
    invalid_event["kind"]["ce"]["specversion"] = json!("0.3");
    let activity: Activity = serde_json::from_value(invalid_event).unwrap();
    assert_eq!(
        activity.validate().unwrap_err().pointer,
        "/kind/ce/specversion"
    );
}

use cmaeg_core::cloudevent::{parse_binary, parse_structured, CloudEvent, MAX_CLOUD_EVENT_BYTES};
use serde_json::json;

fn event() -> CloudEvent {
    serde_json::from_str(include_str!("fixtures/cloudevent.json")).unwrap()
}

fn headers() -> Vec<(String, String)> {
    [
        ("Ce-Specversion", "1.0"),
        ("ce-id", "event-1"),
        ("ce-source", "urn:test:source"),
        ("ce-type", "com.example.test"),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), value.into()))
    .collect()
}

#[test]
fn structured_and_binary_modes_preserve_the_same_event() {
    let mut expected = event();
    expected.time = Some("2026-09-22T00:00:00Z".into());
    expected.subject = Some("subject".into());
    expected.datacontenttype = Some("application/json".into());
    expected.dataschema = Some("https://example.test/schema".into());
    let mut headers = headers();
    for (name, value) in [
        ("time", expected.time.as_ref().unwrap()),
        ("subject", expected.subject.as_ref().unwrap()),
        (
            "datacontenttype",
            expected.datacontenttype.as_ref().unwrap(),
        ),
        ("dataschema", expected.dataschema.as_ref().unwrap()),
    ] {
        headers.push((format!("ce-{name}"), value.clone()));
    }
    headers.push(("ce-traceid".into(), "abc123".into()));
    headers.push(("unrelated".into(), "ignored".into()));
    assert_eq!(parse_binary(&headers, br#"{"value":1}"#).unwrap(), expected);
    assert_eq!(
        parse_structured(&serde_json::to_vec(&expected).unwrap()).unwrap(),
        expected
    );
    assert!(parse_binary(&headers, b"").unwrap().data.is_none());
}

#[test]
fn rejects_missing_binary_required_attributes_individually() {
    for index in 0..4 {
        let mut headers = headers();
        let (name, _) = headers.remove(index);
        let error = parse_binary(&headers, b"{}").unwrap_err();
        assert_eq!(
            error.pointer,
            format!("/{}", name[3..].to_ascii_lowercase())
        );
        assert!(error
            .to_string()
            .contains("required binary attribute missing"));
    }
}

#[test]
fn rejects_invalid_json_and_oversized_binary_data() {
    assert_eq!(parse_structured(b"not JSON").unwrap_err().pointer, "/");
    assert_eq!(
        parse_binary(&headers(), b"not JSON").unwrap_err().pointer,
        "/data"
    );
    assert_eq!(
        parse_binary(&headers(), &vec![b' '; MAX_CLOUD_EVENT_BYTES + 1])
            .unwrap_err()
            .pointer,
        "/data"
    );
    let mut headers = headers();
    headers[0].1 = "0.3".into();
    assert_eq!(
        parse_binary(&headers, b"{}").unwrap_err().pointer,
        "/specversion"
    );
}

#[test]
fn cloud_event_attributes_accept_their_limit_and_refuse_empty_or_overlong_values() {
    for (pointer, max) in [
        ("/id", 512),
        ("/source", 4096),
        ("/type", 512),
        ("/time", 128),
        ("/subject", 4096),
    ] {
        let mut value = serde_json::to_value(event()).unwrap();
        value["time"] = json!("now");
        value["subject"] = json!("subject");
        *value.pointer_mut(pointer).unwrap() = json!("x".repeat(max));
        parse_structured(&serde_json::to_vec(&value).unwrap()).unwrap();
        for size in [0, max + 1] {
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            assert_eq!(
                parse_structured(&serde_json::to_vec(&value).unwrap())
                    .unwrap_err()
                    .pointer,
                pointer
            );
        }
    }
}

#[test]
fn rejects_reserved_or_malformed_extension_names() {
    for name in [
        "",
        "Bad",
        "with_underscore",
        "abcdefghijklmnopqrstu",
        "specversion",
        "id",
        "source",
        "type",
        "time",
        "subject",
        "datacontenttype",
        "dataschema",
        "data",
    ] {
        let mut event = event();
        event.extensions.insert(name.into(), json!("value"));
        assert_eq!(event.validate().unwrap_err().pointer, format!("/{name}"));
    }
    let mut event = event();
    event
        .extensions
        .insert("abcdefghijklmnopqr12".into(), json!(true));
    event.validate().unwrap();
}

#[test]
fn event_size_limit_counts_attributes_and_serialization() {
    let mut event = event();
    event.data = Some(json!(""));
    let overhead = serde_json::to_vec(&event).unwrap().len();
    event.data = Some(json!("x".repeat(MAX_CLOUD_EVENT_BYTES - overhead)));
    let bytes = serde_json::to_vec(&event).unwrap();
    assert_eq!(bytes.len(), MAX_CLOUD_EVENT_BYTES);
    parse_structured(&bytes).unwrap();
    event.data = Some(json!("x".repeat(MAX_CLOUD_EVENT_BYTES - overhead + 1)));
    assert_eq!(event.validate().unwrap_err().pointer, "/");
    // The body alone fits, but the complete binary-mode envelope does not.
    let body = serde_json::to_vec(&"x".repeat(MAX_CLOUD_EVENT_BYTES - 2)).unwrap();
    assert_eq!(parse_binary(&headers(), &body).unwrap_err().pointer, "/");
}

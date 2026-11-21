use super::*;
use crate::telemetry::turn_trace::RemoteParent;
use axum::http::{HeaderMap, HeaderValue};
use serde_json::json;

fn wire() -> Value {
    json!({
        "version":1,
        "turn_id":"11111111-1111-4111-8111-111111111111",
        "trace_id":"11111111111141118111111111111111",
        "parent_span_id":"0123456789abcdef",
        "ingress_request_id":"22222222-2222-4222-8222-222222222222"
    })
}

#[test]
fn canonical_association_requires_exact_version_uuid_span_and_matching_turn() {
    let valid = wire();
    assert_eq!(Association::parse(&valid).unwrap().wire(), valid);
    for (key, value) in [
        ("version", json!(2)),
        ("turn_id", json!("11111111-1111-1111-8111-111111111111")),
        ("turn_id", json!("11111111-1111-4111-c111-111111111111")),
        ("trace_id", json!("22222222222242228222222222222222")),
        ("trace_id", json!("11111111-1111-4111-8111-111111111111")),
        ("parent_span_id", json!("0000000000000000")),
        ("parent_span_id", json!("0123456789ABCDEF")),
        ("ingress_request_id", json!("private-value")),
        ("ingress_request_id", json!("x".repeat(2048))),
        ("unexpected", json!("private-value")),
    ] {
        let mut value_with_error = valid.clone();
        value_with_error[key] = value;
        assert!(Association::parse(&value_with_error).is_none(), "{key}");
    }
    for key in valid.as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(Association::parse(&missing).is_none());
    }
}

#[test]
fn replay_and_fresh_attempt_diagnostics_keep_the_canonical_winner() {
    let first = Association::parse(&wire()).unwrap();
    let mut slot = AssociationSlot::default();
    assert_eq!(slot.bind(first), BindResult::Bound);
    assert_eq!(slot.bind(first), BindResult::Duplicate);
    let other_attempt = Association {
        parent_span_id: Some(SpanId::fresh()),
        ingress_request_id: Uuid::new_v4(),
        ..first
    };
    assert_eq!(slot.bind(other_attempt), BindResult::Conflict);
    assert_eq!(slot.get(), Some(first));
    assert_eq!(
        slot.bind(Association {
            turn_id: Uuid::new_v4(),
            ..first
        }),
        BindResult::Conflict
    );
    assert_eq!(slot.get(), Some(first));
}

#[test]
fn retained_diagnostics_round_trip_or_degrade_to_a_gap_without_raw_tokens() {
    let original = Association::parse(&wire()).unwrap();
    let mut slot = AssociationSlot::default();
    slot.bind(original);
    let encoded = serde_json::to_value(slot).unwrap();
    assert_eq!(encoded, wire());
    let restored: AssociationSlot = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.get(), Some(original));
    for invalid in [
        Value::Null,
        json!("private-retained-trace-canary"),
        json!([wire()]),
        json!({"version":1,"turn_id":"private-retained-trace-canary"}),
    ] {
        let restored: AssociationSlot = serde_json::from_value(invalid).unwrap();
        assert!(restored.is_empty());
        assert_eq!(serde_json::to_value(restored).unwrap(), Value::Null);
    }
}

#[test]
fn header_association_survives_recovery_without_inventing_a_nyxid_parent() {
    let mut header = wire();
    header.as_object_mut().unwrap().remove("parent_span_id");
    // Header correlation is an internal retained form, never a valid protocol
    // annotation without the parent required by the negotiated contract.
    assert!(Association::parse(&header).is_none());
    let retained: AssociationSlot = serde_json::from_value(header.clone()).unwrap();
    assert_eq!(retained.get().unwrap().parent_span_id, None);
    assert_eq!(serde_json::to_value(retained).unwrap(), header);
    let mut recovered = retained;
    let different_turn = Association {
        turn_id: Uuid::new_v4(),
        ..retained.get().unwrap()
    };
    assert_eq!(recovered.bind(different_turn), BindResult::Conflict);
    assert_eq!(recovered.get(), retained.get());
    for value in [Value::Null, json!("private-parent-canary")] {
        let mut malformed = header.clone();
        malformed["parent_span_id"] = value;
        let slot: AssociationSlot = serde_json::from_value(malformed).unwrap();
        assert!(slot.is_empty());
    }
}

#[test]
fn header_context_is_bounded_nonzero_and_single_valued() {
    let good = "00-11111111111141118111111111111111-0123456789abcdef-01";
    let mut headers = HeaderMap::new();
    assert!(RemoteParent::from_headers(&headers).is_none());
    headers.insert("traceparent", good.parse().unwrap());
    let parent = RemoteParent::from_headers(&headers).unwrap();
    assert_eq!(
        parent.trace_id.simple().to_string(),
        "11111111111141118111111111111111"
    );
    assert_eq!(parent.span_id.to_string(), "0123456789abcdef");
    headers.append("traceparent", good.parse().unwrap());
    assert!(RemoteParent::from_headers(&headers).is_none());
    for bad in [
        format!("{good},{good}"),
        "x".repeat(257),
        good.replace("0123456789abcdef", "0000000000000000"),
        good.replace(
            "11111111111141118111111111111111",
            "00000000000000000000000000000000",
        ),
        good.replace("abcdef", "ABCDEF"),
        format!("{good}-private"),
    ] {
        headers.insert("traceparent", HeaderValue::from_str(&bad).unwrap());
        assert!(RemoteParent::from_headers(&headers).is_none());
    }
    headers.insert("traceparent", good.parse().unwrap());
    headers.insert("x-request-id", "private-value".parse().unwrap());
    assert!(RemoteParent::from_headers(&headers)
        .unwrap()
        .ingress_request_id
        .is_none());
    headers.insert(
        "x-request-id",
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
    );
    assert!(RemoteParent::from_headers(&headers)
        .unwrap()
        .ingress_request_id
        .is_some());
    headers.append(
        "x-request-id",
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
    );
    assert!(RemoteParent::from_headers(&headers)
        .unwrap()
        .ingress_request_id
        .is_none());
}

#[test]
fn future_w3c_version_uses_only_the_validated_fixed_prefix() {
    let prefix = "01-11111111111141118111111111111111-0123456789abcdef-03";
    for value in [prefix.to_owned(), format!("{prefix}-opaque-vendor-fields")] {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", value.parse().unwrap());
        assert!(RemoteParent::from_headers(&headers).is_some());
    }
    for value in [
        prefix.replace("01-", "ff-"),
        prefix.replace("01-", "0A-"),
        format!("{prefix}opaque"),
        format!("{prefix}-{}", "x".repeat(257)),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", value.parse().unwrap());
        assert!(RemoteParent::from_headers(&headers).is_none());
    }
}

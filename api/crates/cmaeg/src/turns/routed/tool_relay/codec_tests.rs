use super::codec::{self, Direction, Envelope};
use crate::ingress::routing_plan::fixture::keys;
use serde_json::{json, Value};

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn request(value: &Value) -> Envelope {
    codec::seal(
        &keys(),
        Direction::Request,
        &id(),
        "POST /internal/replicas/tool-reply",
        120,
        100,
        value,
    )
    .unwrap()
}
fn open(envelope: &Envelope) -> Result<Value, crate::turns::routed::ExecutionError> {
    codec::open(
        &keys(),
        Direction::Request,
        "POST /internal/replicas/tool-reply",
        101,
        envelope,
    )
}
fn alter(envelope: &Envelope, mutate: impl FnOnce(&mut Value)) -> Envelope {
    let mut value = serde_json::to_value(envelope).unwrap();
    mutate(&mut value);
    serde_json::from_value(value).unwrap()
}

#[test]
fn bounded_encryption_roundtrips_large_payload_without_exposing_content() {
    let payload = json!({"message": "private-content-marker".repeat(7000)});
    let envelope = request(&payload);
    let wire = serde_json::to_vec(&envelope).unwrap();
    assert!(wire.len() < codec::BODY_LIMIT);
    assert!(!String::from_utf8(wire)
        .unwrap()
        .contains("private-content-marker"));
    assert_eq!(open(&envelope).unwrap(), payload);
}

#[test]
fn relay_rejects_tampered_missing_reordered_and_cross_request_chunks() {
    let a = request(&json!({"message":"a".repeat(70000)}));
    let b = request(&json!({"message":"b".repeat(70000)}));
    let b_value = serde_json::to_value(&b).unwrap();
    for changed in [
        alter(&a, |value| {
            value["chunks"][0] = b_value["chunks"][0].clone()
        }),
        alter(&a, |value| {
            value["chunks"].as_array_mut().unwrap().swap(0, 1)
        }),
        alter(&a, |value| {
            value["chunks"].as_array_mut().unwrap().pop();
        }),
        alter(&a, |value| value["id"] = json!(id())),
        alter(&a, |value| value["manifest"] = b_value["manifest"].clone()),
        alter(&a, |value| value["chunks"][0] = json!("invalid ciphertext")),
    ] {
        assert!(open(&changed).is_err());
    }
    assert_eq!(open(&a).unwrap(), json!({"message":"a".repeat(70000)}));
}

#[test]
fn envelope_binds_direction_route_expiry_and_exact_response_request() {
    let request_id = id();
    let payload = json!({"receipt":"confirmed"});
    let envelope = codec::seal(
        &keys(),
        Direction::Response,
        &request_id,
        "request-sha256",
        120,
        100,
        &payload,
    )
    .unwrap();
    let good: Value = codec::open(
        &keys(),
        Direction::Response,
        "request-sha256",
        119,
        &envelope,
    )
    .unwrap();
    assert_eq!(good, payload);
    for (direction, context, now) in [
        (Direction::Request, "request-sha256", 110),
        (Direction::Response, "another-request-sha256", 110),
        (Direction::Response, "request-sha256", 120),
        (Direction::Response, "request-sha256", 90),
    ] {
        assert!(codec::open::<Value>(&keys(), direction, context, now, &envelope).is_err());
    }
}

#[test]
fn invalid_size_identity_and_lifetime_are_refused_before_relay() {
    for (request_id, expiry, payload) in [
        (id(), 126, json!({})),
        (id(), 100, json!({})),
        ("untrusted-id".into(), 120, json!({})),
        (id(), 120, json!({"message":"x".repeat(192 * 1024)})),
    ] {
        assert!(codec::seal(
            &keys(),
            Direction::Request,
            &request_id,
            "route",
            expiry,
            100,
            &payload
        )
        .is_err());
    }
    let envelope = request(&json!({"valid":true}));
    let oversized = alter(&envelope, |value| value["chunks"] = json!(vec![""; 7]));
    assert!(open(&oversized).is_err());
}

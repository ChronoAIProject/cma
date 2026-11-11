use super::*;

fn receipt() -> Publication {
    let agent_id = "agt_receipt_fixture".to_owned();
    let incarnation = "a".repeat(64);
    let key = "fixture-publication".to_owned();
    Publication {
        id: identity(&agent_id, &incarnation, &key),
        format_version: 1,
        agent_id,
        incarnation,
        publication_key: key,
        fingerprint: request_digest(&("fixture", 1)).unwrap(),
        appended_at: "2026-11-11T00:00:00Z".into(),
        expected_revision: 8,
        expected_seq: 10_000,
        first_seq: 10_001,
        last_seq: 10_002,
        result_revision: 9,
        head_revision: 3,
        active_ordinal: 1,
        previous_seal: Some(digest::binary(vec![7; 32])),
        result_digest: result_digest(&("fixture", 2)).unwrap(),
    }
}

#[test]
fn receipts_require_every_binding_and_accept_only_bounded_intervals() {
    let original = receipt();
    original.validate().unwrap();
    let document = bson::serialize_to_document(&original).unwrap();
    Publication::read(document.clone()).unwrap();
    for field in document.keys() {
        let mut missing = document.clone();
        missing.remove(field);
        assert!(Publication::read(missing).is_err(), "missing {field}");
    }
    let mut empty = original.clone();
    empty.last_seq = empty.expected_seq;
    empty.validate().unwrap();
    let mut too_big = original.clone();
    too_big.last_seq = too_big.expected_seq + 65;
    assert!(too_big.validate().is_err());
    let mut wrong_interval = original.clone();
    wrong_interval.first_seq += 1;
    assert!(wrong_interval.validate().is_err());
    let mut unknown_version = document;
    unknown_version.insert("format_version", 2_i64);
    assert!(Publication::read(unknown_version).is_err());
}

#[test]
fn receipt_rejects_invalid_digest_timestamp_identity_and_revision() {
    let original = receipt();
    for change in [
        |r: &mut Publication| r.fingerprint.bytes.pop().map(|_| ()).unwrap(),
        |r: &mut Publication| r.result_digest.bytes.clear(),
        |r: &mut Publication| r.appended_at = "invalid".into(),
        |r: &mut Publication| r.incarnation = "z".repeat(64),
        |r: &mut Publication| r.id.push('x'),
        |r: &mut Publication| r.result_revision = 7,
        |r: &mut Publication| r.head_revision = 0,
        |r: &mut Publication| r.previous_seal = None,
        |r: &mut Publication| r.last_seq = MAX_SEQUENCE + 1,
    ] {
        let mut changed = original.clone();
        change(&mut changed);
        assert!(changed.validate().is_err());
    }
}

#[test]
fn canonical_request_and_result_digests_preserve_values_and_separate_domains() {
    let mut left = serde_json::Map::new();
    left.insert("b".into(), serde_json::json!({"z":2,"a":1}));
    left.insert("a".into(), serde_json::json!([2, 1]));
    let right = serde_json::json!({"a":[2,1],"b":{"a":1,"z":2}});
    assert_eq!(
        request_digest(&left).unwrap(),
        request_digest(&right).unwrap()
    );
    assert_ne!(
        request_digest(&left).unwrap(),
        result_digest(&left).unwrap()
    );
    assert_ne!(
        request_digest(&left).unwrap(),
        request_digest(&serde_json::json!({"a":[1,2],"b":{"a":1,"z":2}})).unwrap()
    );
    assert_ne!(identity("ab:c", "d", "e"), identity("ab", "c:d", "e"));
    assert_ne!(identity("ab", "c", "d:e"), identity("ab", "c:d", "e"));
}

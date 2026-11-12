use super::*;
use crate::allocation_probe::AllocationProbe;
use serde_json::json;

#[test]
fn existing_byte_control_and_empty_rules_preserve_all_accepted_values() {
    for session in ["s".into(), " ".into(), "é".repeat(512), "x".repeat(1024)] {
        let result = json!({"sessionId":session});
        for bounded in [false, true] {
            let decoded = read(&result, bounded).unwrap();
            validate(&decoded).unwrap();
            assert_eq!(decoded, session);
        }
    }
    for session in [
        "".into(),
        "x".repeat(1025),
        "é".repeat(513),
        "a\nb".into(),
        "\0".into(),
        "\u{85}".into(),
    ] {
        let result = json!({"sessionId":session});
        for bounded in [false, true] {
            let error = read(&result, bounded)
                .and_then(|id| validate(&id))
                .unwrap_err();
            assert!(
                matches!(error, AdapterError::Rejected(message) if message == "bridge returned an invalid conversation identifier")
            );
        }
    }
}

#[test]
fn absent_and_wrong_type_identity_preserve_fixed_missing_identity_error() {
    for result in [
        json!({}),
        json!({"sessionId":null}),
        json!({"sessionId":12}),
        json!({"sessionId":["s"]}),
        json!({"sessionId":{"private":"value"}}),
        Value::Null,
    ] {
        for bounded in [false, true] {
            assert!(
                matches!(read(&result, bounded), Err(AdapterError::Rejected(message)) if message == "bridge named no conversation")
            );
        }
    }
}

#[test]
fn maximal_invalid_reply_is_refused_before_copying_its_identity() {
    let mut outer = json!({"id":"rpc","result":{"sessionId":""}});
    let length = 4 * 1024 * 1024 - serde_json::to_vec(&outer).unwrap().len();
    outer["result"]["sessionId"] = json!("x".repeat(length));
    let encoded = serde_json::to_vec(&outer).unwrap();
    assert_eq!(encoded.len(), 4 * 1024 * 1024);
    crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    for bounded in [false, true] {
        let mut probe = AllocationProbe::new();
        // Measure projection plus the common identity rule; other activation
        // clones and the previously admitted JSON tree are separate owners.
        let error = probe.track(|| {
            read(&outer["result"], bounded)
                .and_then(|id| validate(&id))
                .unwrap_err()
        });
        assert!(matches!(error, AdapterError::Rejected(_)));
        let peak = probe.peak();
        if bounded {
            assert!(peak < 1024);
        } else {
            assert!(peak >= length);
        }
        drop(error);
        assert_eq!(probe.live(), 0);
        eprintln!("Session identity allocation bounded={bounded}: encoded={} identity={length} peak={peak} final=0",encoded.len());
    }
}

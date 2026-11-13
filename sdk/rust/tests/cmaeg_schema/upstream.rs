use super::*;

#[test]
fn refusal_readers_accept_the_published_vocabulary_and_http_boundaries() {
    let schema = snapshot();
    let contract = &schema["components"]["schemas"]["UpstreamRefusal"]["properties"];
    assert_eq!(contract["status"]["minimum"], 100);
    assert_eq!(contract["status"]["maximum"], 599);
    let operations = contract["operation"]["enum"].as_array().unwrap();
    let codes = contract["code"]["enum"].as_array().unwrap();
    assert_eq!(operations, &vec![json!("responses"), json!("conversation")]);
    assert_eq!(codes.len(), 12);
    for operation in operations {
        for code in codes {
            for status in [100, 403, 599] {
                let upstream = json!({"operation":operation,"code":code,"status":status});
                let parsed: UpstreamRefusal = serde_json::from_value(upstream.clone()).unwrap();
                assert_eq!(serde_json::to_value(parsed).unwrap(), upstream);
                let mut problem = fixtures()["Problem"][0].clone();
                problem["upstream"] = upstream;
                let parsed: Problem = serde_json::from_value(problem.clone()).unwrap();
                assert_eq!(serde_json::to_value(parsed).unwrap(), problem);
            }
        }
    }
}

fn rejected(upstream: Value) {
    let error = serde_json::from_value::<UpstreamRefusal>(upstream.clone())
        .err()
        .expect("refusal must reject malformed metadata");
    assert!(!error.to_string().contains("private"));
    let mut problem = fixtures()["Problem"][0].clone();
    problem["upstream"] = upstream;
    let error = serde_json::from_value::<Problem>(problem)
        .err()
        .expect("problem must reject malformed metadata");
    assert!(!error.to_string().contains("private"));
}

#[test]
fn refusal_readers_reject_private_or_unknown_codes_and_operations() {
    for code in [
        "private-account-identifier_42".to_owned(),
        "media_source_refused.private-value".to_owned(),
        "MEDIA_SOURCE_REFUSED".to_owned(),
        String::new(),
        "x".repeat(4096),
    ] {
        rejected(json!({"operation":"responses","code":code,"status":403}));
    }
    for operation in ["private-operation", "Responses", "", "response"] {
        rejected(json!({"operation":operation,"code":"unrecognized","status":403}));
    }
}

#[test]
fn refusal_readers_reject_invalid_http_statuses_in_both_entry_points() {
    for status in [
        json!(-1),
        json!(0),
        json!(99),
        json!(600),
        json!(65535),
        json!(65536),
        json!(9007199254740991_u64),
        json!(100.5),
        json!("403"),
        Value::Null,
    ] {
        rejected(json!({"operation":"responses","code":"unrecognized","status":status}));
    }
}

#[test]
fn refusal_metadata_roundtrips_without_changing_the_legacy_problem() {
    let legacy = fixtures()["Problem"][0].clone();
    assert!(legacy.get("upstream").is_none());
    let decoded: Problem = serde_json::from_value(legacy.clone()).unwrap();
    assert!(decoded.upstream.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);

    for refusal in fixtures()["UpstreamRefusal"].as_array().unwrap() {
        let mut enriched = legacy.clone();
        enriched["upstream"] = refusal.clone();
        let decoded: Problem = serde_json::from_value(enriched.clone()).unwrap();
        let upstream = decoded.upstream.as_ref().unwrap();
        assert_eq!(upstream.status, refusal["status"].as_u64().unwrap() as u16);
        assert_eq!(serde_json::to_value(decoded).unwrap(), enriched);
    }
    for malformed in [
        json!({}),
        json!({"operation":"responses","status":"403","code":"unrecognized"}),
    ] {
        let mut enriched = legacy.clone();
        enriched["upstream"] = malformed;
        assert!(serde_json::from_value::<Problem>(enriched).is_err());
    }
}

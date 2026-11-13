use super::*;

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

use cma_sdk::cmaeg::types::ProviderBinding;
use serde_json::{Value, json};

#[test]
fn provider_binding_preserves_legacy_absence_and_acknowledged_capability() {
    let legacy = json!({"binding_id":"bnd_synthetic","level":"l1","slug":"nyxbot"});
    let decoded: ProviderBinding = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
    let mut negotiated = legacy.clone();
    negotiated["capabilities"] = json!(["nyxid_turn_trace_v1"]);
    let decoded: ProviderBinding = serde_json::from_value(negotiated.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), negotiated);
    for invalid in [
        json!("nyxid_turn_trace_v1"),
        json!([42]),
        json!({}),
        Value::Null,
    ] {
        let mut invalid_binding = legacy.clone();
        invalid_binding["capabilities"] = invalid;
        assert!(serde_json::from_value::<ProviderBinding>(invalid_binding).is_err());
    }
}

use cma_sdk::cmaeg::types::{CapabilityView, RelayAuthoringCapabilities};
use serde_json::{Value, json};

fn legacy() -> Value {
    json!({
        "dynamic_sources_writes_enabled": true,
        "relay_raw_verification_writes_enabled": true,
        "multi_consumer_writes_enabled": true,
        "native_bot_mentions_writes_enabled": false,
        "thread_contract_writes_enabled": false,
        "native_discord_writes_enabled": true,
        "native_discord_reactions_enabled": true,
        "platforms": []
    })
}

#[test]
fn legacy_discovery_does_not_infer_discord_forwards_support() {
    let value = legacy();
    let decoded: RelayAuthoringCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded.native_discord_forwards_enabled, None);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    let view = json!({"platforms": [], "relay_authoring": value});
    let decoded: CapabilityView = serde_json::from_value(view.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), view);
}

#[test]
fn explicit_forwards_discovery_roundtrips_and_malformed_values_are_rejected() {
    for enabled in [false, true] {
        let mut value = legacy();
        value["native_discord_forwards_enabled"] = json!(enabled);
        let decoded: RelayAuthoringCapabilities = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(decoded.native_discord_forwards_enabled, Some(enabled));
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
        let view = json!({"platforms": [], "relay_authoring": value});
        let decoded: CapabilityView = serde_json::from_value(view.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), view);
    }
    let mut value = legacy();
    value["native_discord_forwards_enabled"] = Value::Null;
    let decoded: RelayAuthoringCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded.native_discord_forwards_enabled, None);
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy());
    for enabled in [json!("true"), json!(1), json!({}), json!([])] {
        value["native_discord_forwards_enabled"] = enabled;
        assert!(serde_json::from_value::<RelayAuthoringCapabilities>(value.clone()).is_err());
    }
}

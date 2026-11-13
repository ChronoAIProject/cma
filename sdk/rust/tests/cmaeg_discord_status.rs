use cma_sdk::cmaeg::types::NativeDiscordActivation;
use serde_json::json;

#[test]
fn selected_legacy_response_does_not_invent_connection_health() {
    let legacy = json!({"state":"selected","detail":"selected","capacity":64,"selected_records":2});
    let parsed: NativeDiscordActivation = serde_json::from_value(legacy.clone()).unwrap();
    assert!(parsed.connection.is_none());
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
}
#[test]
fn connection_status_keeps_null_unknown_and_explicit_failure() {
    for health in [None, Some(false), Some(true)] {
        let value = json!({"state":"selected","detail":"selected","capacity":64,"selected_records":2,
            "connection":{"scope":"serving_instance","stage":"ready","outcome":"ok",
            "connected":health,"authenticated":health,"ready":health,"lease":"acquired",
            "identify":"sent","stale":false,"last_dispatch_outcome":"unknown",
            "last_failure_stage":"lease","last_failure_code":"lease_contended"}});
        let parsed: NativeDiscordActivation = serde_json::from_value(value).unwrap();
        let status = parsed.connection.unwrap();
        assert_eq!(status.connected, health);
        assert_eq!(status.last_failure_code.as_deref(), Some("lease_contended"));
    }
}

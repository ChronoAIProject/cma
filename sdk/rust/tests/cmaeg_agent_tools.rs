use cma_sdk::cmaeg::types::CapabilityView;
use serde_json::json;

#[test]
fn agent_tool_catalog_is_optional_and_accepts_future_server_tool_names() {
    for value in [
        json!({"platforms":[]}),
        json!({"platforms":[],"agent_tools":null}),
    ] {
        let decoded: CapabilityView = serde_json::from_value(value).unwrap();
        assert!(decoded.agent_tools.is_none());
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            json!({"platforms":[]})
        );
    }
    for tools in [
        json!([]),
        json!([{"name":"futureTool","description":"A server-advertised tool.","read_only":false,"destructive":true,"idempotent":true,"open_world":true}]),
    ] {
        let value = json!({"platforms":[],"agent_tools":tools});
        let decoded: CapabilityView = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
}

use cma_sdk::cmaeg::types::{Source, SourceKind, SourceRequest, WhatsappCredentials};
use serde_json::json;

#[test]
fn retained_native_sources_keep_thread_history_absent() {
    let value = json!({"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}});
    let source: Source = serde_json::from_value(value.clone()).unwrap();
    let request: SourceRequest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(source).unwrap(), value);
    assert_eq!(serde_json::to_value(request).unwrap(), value);
}

#[test]
fn native_whatsapp_source_and_private_bundle_keep_exact_wire_facts() {
    let value = json!({"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","admission":{"type":"all"}});
    let source: Source = serde_json::from_value(value.clone()).unwrap();
    let request: SourceRequest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(source).unwrap(), value);
    assert_eq!(serde_json::to_value(request).unwrap(), value);
    let kind =
        json!({"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567"});
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<SourceKind>(kind.clone()).unwrap()).unwrap(),
        kind
    );
    let credentials = json!({"app_secret":"fixture-secret","access_token":"fixture-token","verify_token":"fixture-verify"});
    let decoded: WhatsappCredentials = serde_json::from_value(credentials.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), credentials);
}

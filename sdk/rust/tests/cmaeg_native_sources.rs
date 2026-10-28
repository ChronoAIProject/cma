use cma_sdk::cmaeg::types::{
    RelayAuthoringCapabilities, Source, SourceKind, SourceRequest, WhatsappCredentials,
};
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

#[test]
fn native_whatsapp_capabilities_never_infer_support_from_legacy_gates() {
    let legacy = json!({"dynamic_sources_writes_enabled":true,"relay_raw_verification_writes_enabled":true,"multi_consumer_writes_enabled":true,"native_bot_mentions_writes_enabled":false,"thread_contract_writes_enabled":false,"platforms":[]});
    let parsed: RelayAuthoringCapabilities = serde_json::from_value(legacy.clone()).unwrap();
    assert!(parsed.native_whatsapp_writes_enabled.is_none());
    assert!(parsed.native_whatsapp_groups_writes_enabled.is_none());
    assert!(parsed.native_whatsapp_auto_activation_enabled.is_none());
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
    for enabled in [false, true] {
        let mut value = legacy.clone();
        value["native_whatsapp_writes_enabled"] = enabled.into();
        value["native_whatsapp_auto_activation_enabled"] = enabled.into();
        let parsed: RelayAuthoringCapabilities = serde_json::from_value(value.clone()).unwrap();
        assert!(parsed.native_whatsapp_groups_writes_enabled.is_none());
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        value["native_whatsapp_groups_writes_enabled"] = (!enabled).into();
        let parsed: RelayAuthoringCapabilities = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.native_whatsapp_groups_writes_enabled, Some(!enabled));
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
}

#[test]
fn native_whatsapp_group_opt_in_preserves_opaque_ids_in_sources_and_requests() {
    let value = json!({
        "type":"whatsapp", "app_id":"12345", "waba_id":"23456",
        "phone_number_id":"34567", "group_ids":["opaque+group/id==", "another_group-id"],
        "admission":{"type":"all"}
    });
    let source: Source = serde_json::from_value(value.clone()).unwrap();
    let request: SourceRequest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(source).unwrap(), value);
    assert_eq!(serde_json::to_value(request).unwrap(), value);
}

#[test]
fn separate_native_lark_source_and_credentials_round_trip_without_relay_authority() {
    for region in ["lark", "feishu"] {
        let identity = json!({"region":region,"app_id":"cli_separate_app","tenant_key":"native_tenant","bot_open_id":"ou_native_bot"});
        let source = json!({"type":"native_lark","separate_app":true,"identity":identity,"admission":{"type":"all"}});
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<Source>(source.clone()).unwrap())
                .unwrap(),
            source
        );
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<SourceRequest>(source.clone()).unwrap())
                .unwrap(),
            source
        );
        let mut activity = identity;
        activity["type"] = "native_lark".into();
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<SourceKind>(activity.clone()).unwrap())
                .unwrap(),
            activity
        );
    }
    let value = json!({"app_secret":"fixture-secret","encrypt_key":"fixture-encrypt","verification_token":"fixture-token"});
    let credentials: cma_sdk::cmaeg::types::NativeLarkCredentials =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(credentials).unwrap(), value);
}

#[test]
fn native_telegram_channel_posts_are_an_independent_optional_source_opt_in() {
    let original = json!({"type":"telegram","bot_id":"77","admission":{"type":"all"}});
    for opted in [false, true] {
        let mut value = original.clone();
        if opted {
            value["message_events"] = true.into();
            value["channel_posts"] = true.into();
        }
        let source: Source = serde_json::from_value(value.clone()).unwrap();
        let request: SourceRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(source).unwrap(), value);
        assert_eq!(serde_json::to_value(request).unwrap(), value);
    }
}

#[test]
fn native_whatsapp_incoming_changes_consent_is_optional_and_independent() {
    let legacy = json!({"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","admission":{"type":"all"}});
    for enabled in [false, true] {
        let mut value = legacy.clone();
        value["message_events"] = enabled.into();
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<Source>(value.clone()).unwrap()).unwrap(),
            value
        );
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<SourceRequest>(value.clone()).unwrap())
                .unwrap(),
            value
        );
    }
}

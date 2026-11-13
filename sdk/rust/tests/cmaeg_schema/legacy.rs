use super::*;

#[test]
fn telegram_rename_keeps_strict_legacy_discovery_bytes_and_existing_request_shape() {
    let legacy =
        r#"{"manage_original_topic":true,"actions":["close","reopen"],"expires_at":1800000600}"#;
    let caps: TopicLifecycleCapabilities = serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_string(&caps).unwrap(), legacy);
    assert!(
        serde_json::from_str::<TopicLifecycleCapabilities>(&legacy.replace("reopen", "rename"))
            .is_err()
    );
    let request = r#"{"target":{"type":"event","event_ref":"cmaeg1.ev.fixture"},"action":"rename","name":"Incident triage"}"#;
    let decoded: ManageThreadRequest = serde_json::from_str(request).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), request);
}

#[test]
fn historical_activity_receipts_roundtrip_without_inventing_event_facts() {
    let mut page = fixtures()["ActivityPage"][0].clone();
    for row in page["items"].as_array_mut().unwrap() {
        for key in ["kind", "event_type", "source_occurred_at"] {
            row.as_object_mut().unwrap().remove(key);
        }
    }
    let decoded: ActivityPage = serde_json::from_value(page.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), page);
}

#[test]
fn github_receipt_dispatch_progress_is_additive_and_nullable() {
    let mut receipt = fixtures()["Receipt"][0].clone();
    receipt["dispatch"] = json!([{
        "channel_id":"cmaeg1.ch.example",
        "status":"dispatched",
        "reason":null,
        "admission_id":"admission-1"
    }]);
    let decoded: Receipt = serde_json::from_value(receipt.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), receipt);
}

#[test]
fn historical_media_capabilities_do_not_advertise_native_presentations() {
    let mut legacy = fixtures()["NativeMediaCapabilities"][0].clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("presentation_profiles");
    let decoded: NativeMediaCapabilities = serde_json::from_value(legacy.clone()).unwrap();
    assert!(decoded.presentation_profiles.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}

#[test]
fn legacy_relay_sources_never_gain_a_resource_binding_on_roundtrip() {
    for platform in ["telegram", "lark", "feishu", "discord", "whatsapp", "slack"] {
        let request = json!({"type":"nyxid_relay","platform":platform,"issuer":"https://nyx.example","key_id":"key-fixture","route_ids":["route-fixture"],"admission":{"type":"all"}});
        let decoded: SourceRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), request);
        let mut source = request;
        source["jwks_url"] = json!("https://nyx.example/.well-known/jwks.json");
        let decoded: Source = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), source);
    }
}

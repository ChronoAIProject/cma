use super::*;

#[test]
fn control_classification_requires_original_verified_human_text_on_each_supported_platform() {
    for platform in platforms() {
        let mut fixture = Fixture::new(platform.clone());
        let text = "CMA-CONTROL/1 cancel ccr_source";
        fixture.body["content"]["text"] = json!(text);
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                fixture.raw_mut()["message"]["text"] = json!(text);
            }
            BridgePlatform::Discord => {
                fixture.raw_mut()["d"]["content"] = json!(text);
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                fixture.raw_mut()["event"]["message"]["content"] =
                    json!(json!({"text":text}).to_string());
            }
        }
        let write = fixture.write().unwrap();
        assert_eq!(
            write.control_refusal(),
            Some(ChannelControlRefusal::ChannelControlsDisabled)
        );
        // A newly signed mismatch still lacks the trusted raw-text contract.
        fixture.body["content"]["text"] = json!("CMA-CONTROL/1 approve ccr_source");
        let denied = fixture.write().unwrap();
        assert_ne!(denied.record().admission, InboxAdmission::Accepted);
        assert_eq!(denied.control_refusal(), None);
    }
}

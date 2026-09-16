use super::*;

pub(in crate::store::bridge_definitions::mongo_integration_tests) fn callback_for_platform(
    message: &str,
    text: &str,
    platform: &BridgePlatform,
) -> (Vec<u8>, String, String) {
    if *platform == BridgePlatform::Telegram {
        return callback(message, text);
    }
    assert!(matches!(
        platform,
        BridgePlatform::Lark | BridgePlatform::Feishu
    ));
    let platform = serde_json::to_value(platform).unwrap();
    let body = serde_json::to_vec(&json!({
        "message_id":message,"correlation_id":"callback-1","platform":platform,
        "agent":{"api_key_id":"key-1","name":"Fixture"},
        "conversation":{"id":"route-1","platform_id":"-100123","type":"group"},
        "sender":{"platform_id":"12345"},"content":{"type":"text","text":text},
        "timestamp":"2026-09-15T00:00:00Z",
        "raw_platform_data":{"schema":"2.0","header":{"event_type":"im.message.receive_v1"},
            "event":{"sender":{"sender_id":{"open_id":"12345"},"sender_type":"user"},
                "message":{"message_id":"om_fixture","chat_id":"-100123","chat_type":"group",
                    "message_type":"text","content":json!({"text":text}).to_string()}}}
    }))
    .unwrap();
    let signed = sign(json!({
        "iss":"https://nyx.example","aud":"channel-relay/callback","iat":now(),"exp":now()+300,
        "jti":"callback-1","token_type":"relay_callback","api_key_id":"key-1",
        "message_id":message,"platform":platform,"body_sha256":format!("{:x}",Sha256::digest(&body))
    }));
    (body, signed, String::new())
}

#[path = "support/wire.rs"]
mod wire;
use cma_sdk::cmaeg::{
    Cmaeg,
    types::{MessageEffectsCapabilities, NativeMediaUpdate, OutboundMessage},
};
use serde_json::json;

#[test]
fn patch_roundtrip_preserves_clear_and_rejects_null() {
    let message = json!({"native_media_update":{"caption":""}});
    let decoded: OutboundMessage = serde_json::from_value(message.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), message);
    for field in ["replacement", "caption", "text"] {
        assert!(serde_json::from_value::<NativeMediaUpdate>(json!({field:null})).is_err());
    }
    assert!(
        serde_json::from_value::<OutboundMessage>(json!({"native_media_update":null})).is_err()
    );
    let legacy = json!({"delete":true,"reaction":true,"scope":"confirmed_native_bot_message","max_reactions":1});
    let decoded: MessageEffectsCapabilities = serde_json::from_value(legacy.clone()).unwrap();
    assert!(decoded.media_edit.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}

#[tokio::test]
async fn keyed_update_sends_once_and_retains_next_reference() {
    let server = wire::WireServer::new().await;
    let _ = (&server.client, &server.anonymous);
    let client = Cmaeg::builder(&server.base_url)
        .unwrap()
        .bearer_token("sdk-fixture")
        .unwrap()
        .build()
        .unwrap();
    let body = json!({"native_media_update":{"caption":"","text":"visible content"}});
    let response = json!({"delivery":{"outcome":"confirmed"},"rendering":{"outcome":"rendered"},"message_ref":"next/revision"});
    let mut expected = wire::json(
        "POST",
        "/v1/messages/opaque%2Fref/update",
        Some(body.clone()),
        200,
        response.clone(),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "media-edit-1".into());
    server.expect(expected);
    let result = client
        .update_message_with_idempotency(
            "opaque/ref",
            "media-edit-1",
            &serde_json::from_value(body).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), response);
    server.assert_drained();
    assert_eq!(server.request_count(), 1);
}

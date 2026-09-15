//! Genuine signed callbacks and disposable Mongo storage; no live provider effects.
use super::*;
use crate::store::bridge_definitions::inbox::receipts::model::{
    AcceptanceDisposition, AcceptedCallbackReceipt,
};

async fn proven_write(
    store: &MongoBridgeStore,
    id: &BridgeId,
    body: &[u8],
    token: &str,
) -> InboxWrite {
    let bridge = store.bridge(id).await.unwrap().unwrap();
    let proof = Authority
        .validate_connection(&bridge.owner, &bridge.current().unwrap().config)
        .await
        .unwrap();
    let binding = CallbackBinding::new(bridge.clone(), proof).unwrap();
    let keys = CallbackKeySet::from_pinned_jwks(
        "https://nyx.example".into(),
        JWKS,
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let verifier = CallbackVerifier::new(CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    })
    .unwrap();
    let wire: Value = serde_json::from_slice(body).unwrap();
    assert!(wire
        .pointer("/raw_platform_data/message/reply_to_message")
        .is_none());
    assert!(wire.get("reply_to_platform_message_id").is_none());
    let signed = verifier
        .preauthenticate(
            bridge,
            &keys,
            CallbackHeaders {
                callback_token: token,
                user_token: None,
                message_id: wire["message_id"].as_str().unwrap(),
                platform: "telegram",
                timestamp: &layer2_core::now_rfc3339(),
            },
            body,
        )
        .unwrap();
    let payload = signed.authenticated_payload(body).unwrap();
    let facts = crate::bridge::ingress::NyxidCapabilities
        .observe(&payload)
        .unwrap();
    let descriptor = VerifiedPlatformRedelivery::from_verified_observation(
        &payload,
        &facts,
        VerifiedRedeliveryParent::Absent,
    )
    .unwrap();
    assert!(descriptor.is_some());
    signed
        .classify_capabilities(body, &crate::bridge::ingress::NyxidCapabilities)
        .unwrap()
        .bind(binding)
        .unwrap()
        .with_platform_redelivery(descriptor)
        .unwrap()
}

#[path = "inbox_redelivery/failure_tests.rs"]
mod failure_tests;
#[path = "inbox_redelivery/receipt_tests.rs"]
mod receipt_tests;

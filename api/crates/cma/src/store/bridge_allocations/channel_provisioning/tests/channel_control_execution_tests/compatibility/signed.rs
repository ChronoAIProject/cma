use super::*;
use crate::bridge::ingress::reply_addressing::{ReplyCapabilities, ReplyLookup};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::bridge::outbox::OutboxItem;
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn body(callback: &str, provider: u64, text: &str, reply: bool) -> Value {
    let (body, _, _) =
        crate::store::bridge_definitions::mongo_integration_tests::inbox::callback_for_sender(
            callback, text, 12345,
        );
    let mut value: Value = serde_json::from_slice(&body).unwrap();
    // The cross-process scenario file carries only fixture input, never tokens
    // or protected reply/thread credentials. Each process signs locally.
    value.as_object_mut().unwrap().remove("reply_token");
    value.as_object_mut().unwrap().remove("thread_id");
    value["raw_platform_data"]
        .as_object_mut()
        .unwrap()
        .remove("secret");
    value["correlation_id"] = json!(callback);
    value["raw_platform_data"]["message"]["message_id"] = json!(provider);
    if reply {
        value["reply_to_platform_message_id"] = json!("55");
        value["raw_platform_data"]["message"]["reply_to_message"] = json!({"message_id":55,"chat":{"id":-100123,"type":"supergroup"},"from":{"id":999,"is_bot":true}});
    }
    value
}

pub(super) async fn write(
    client: &Client,
    database: &str,
    bridges: &MongoBridgeStore,
    bridge_id: &BridgeId,
    value: &Value,
) -> InboxWrite {
    let body = serde_json::to_vec(value).unwrap();
    let callback = value["message_id"].as_str().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = sign(
        json!({"iss":"https://nyx.example","aud":"channel-relay/callback","iat":now,"exp":now+300,
        "jti":callback,"token_type":"relay_callback","api_key_id":"key-1","message_id":callback,"platform":"telegram",
        "body_sha256":format!("{:x}",Sha256::digest(&body))}),
    );
    let bridge = bridges.bridge(bridge_id).await.unwrap().unwrap();
    let binding = CallbackBinding::new(
        bridge.clone(),
        Authority
            .validate_connection(&bridge.owner, &bridge.current().unwrap().config)
            .await
            .unwrap(),
    )
    .unwrap();
    let keys = CallbackKeySet::from_pinned_jwks(
        "https://nyx.example".into(),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../layer2-core/src/bridge/ingress/fixtures/jwks.json"
        )),
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let signed = CallbackVerifier::new(CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    })
    .unwrap()
    .preauthenticate(
        bridge,
        &keys,
        CallbackHeaders {
            callback_token: &token,
            user_token: None,
            message_id: callback,
            platform: "telegram",
            timestamp: &layer2_core::now_rfc3339(),
        },
        &body,
    )
    .unwrap();
    let payload = signed.authenticated_payload(&body).unwrap();
    let lookup = ReplyLookup::authenticated(&payload).unwrap();
    let confirmed = if let Some(lookup) = lookup {
        let row = client
            .database(database)
            .collection::<Document>("bridge_outbox_items")
            .find_one(doc! {"state":"sent"})
            .await
            .unwrap()
            .unwrap();
        let item: OutboxItem = crate::store::sandbox_mongo::read_record(&row).unwrap();
        lookup.confirm(&item).unwrap()
    } else {
        None
    };
    let extractor = ReplyCapabilities(confirmed);
    let facts = extractor.observe(&payload).unwrap();
    let parent = if value.get("reply_to_platform_message_id").is_some() {
        VerifiedRedeliveryParent::Message("55".into())
    } else {
        VerifiedRedeliveryParent::Absent
    };
    let provider =
        VerifiedPlatformRedelivery::from_verified_observation(&payload, &facts, parent).unwrap();
    signed
        .classify_capabilities(&body, &extractor)
        .unwrap()
        .bind(binding)
        .unwrap()
        .with_platform_redelivery(provider)
        .unwrap()
}

fn sign(claims: Value) -> String {
    let pair = RsaKeyPair::from_pkcs8(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../layer2-core/src/bridge/ingress/fixtures/callback-test-only.pk8"
    )))
    .unwrap();
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT","kid":"fixture-key"}"#),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let mut signature = vec![0; pair.public().modulus_len()];
    pair.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

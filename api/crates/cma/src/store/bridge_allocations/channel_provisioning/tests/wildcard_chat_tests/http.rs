use super::*;
use crate::bridge::ingress::{CallbackIngress, PinnedCallbackKeys};
use axum::{body::Body, http::Request, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

pub(super) struct SignedChat {
    pub message: String,
    pub chat: String,
    pub body: Vec<u8>,
}

impl SignedChat {
    pub fn new(message: &str, chat: i64) -> Self {
        let (body, _, _) =
            crate::store::bridge_definitions::mongo_integration_tests::inbox::callback(
                message,
                "Keep this chat's work separate.",
            );
        let mut body: Value = serde_json::from_slice(&body).unwrap();
        // NyxID build_callback_payload retains the route record ID separately
        // from the actual inbound chat for an upstream default/wildcard route.
        body["correlation_id"] = json!(message);
        body["conversation"]["platform_id"] = json!(chat.to_string());
        body["raw_platform_data"]["message"]["chat"]["id"] = json!(chat);
        assert_eq!(body["conversation"]["id"], "route-1");
        assert_eq!(body["sender"]["platform_id"], "12345");
        assert_eq!(body["raw_platform_data"]["message"]["message_id"], 42);
        Self {
            message: message.into(),
            chat: chat.to_string(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    pub fn request(&self, bridge: &BridgeId) -> Request<Body> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({"iss":"https://nyx.example","aud":"channel-relay/callback","iat":now,"exp":now+300,
            "jti":self.message,"token_type":"relay_callback","api_key_id":"key-1","message_id":self.message,
            "platform":"telegram","body_sha256":format!("{:x}", Sha256::digest(&self.body))});
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
        Request::post(format!("/api/v1/bridge-callbacks/{}", bridge.as_str()))
            .header("content-type", "application/json")
            .header(
                "x-nyxid-callback-token",
                format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)),
            )
            .header("x-nyxid-message-id", &self.message)
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-timestamp", layer2_core::now_rfc3339())
            .body(Body::from(self.body.clone()))
            .unwrap()
    }
}

pub(super) async fn router(s: &fixture::Setup) -> Router {
    // Reopening reconstructs both stores and ingress from durable state.
    let bridges = Arc::new(open_store(&s.client, &s.database).await);
    let inbox = Arc::new(
        crate::store::MongoCallbackInbox::open(
            &bridges,
            &s.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    crate::bridge::ingress::http::router(Some(Arc::new(
        CallbackIngress::new(
            bridges,
            Arc::new(Authority),
            inbox,
            PinnedCallbackKeys::fixture(),
            CallbackTokenPolicy {
                issuer: "https://nyx.example".into(),
                relay_audience: "https://nyx.example".into(),
                callback_ttl_seconds: 300,
                relay_ttl_seconds: 300,
                reply_ttl_seconds: 1800,
                clock_skew_seconds: 60,
            },
            "https://cma.example/api/v1/bridge-callbacks".into(),
        )
        .unwrap(),
    )))
}

pub(super) async fn accept(
    router: Router,
    s: &fixture::Setup,
    input: &SignedChat,
) -> (Value, StoredInbox) {
    let response = router.oneshot(input.request(&s.id)).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    let response: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 32768)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["status"], "inbox_persisted");
    let id = response["receipt"]["inbox_id"].as_str().unwrap();
    let row = s
        .client
        .database(&s.database)
        .collection::<Document>("bridge_callback_inbox")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    let inbox: StoredInbox = crate::store::sandbox_mongo::read_record(&row).unwrap();
    assert_eq!(inbox.id, id);
    assert_eq!(inbox.message_id, input.message);
    assert_eq!(inbox.envelope.chat_id, input.chat);
    assert_eq!(inbox.envelope.sender_id, "12345");
    assert_eq!(inbox.envelope.route_id, "route-1");
    assert_eq!(inbox.bot_id, "existing-bot");
    assert_eq!(inbox.attestation.issuer, "https://nyx.example");
    assert_eq!(inbox.envelope.conversation_type, "group");
    assert_eq!(inbox.admission, InboxAdmission::Accepted);
    let bridge = s.bridges.bridge(&s.id).await.unwrap().unwrap();
    assert_eq!(
        NormalizedPartitionSource::from_stored(&inbox, &bridge)
            .unwrap()
            .provider_message_id(),
        Some("42")
    );
    (response, inbox)
}

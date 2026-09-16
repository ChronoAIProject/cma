use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use layer2_core::bridge::{allocation::*, ingress::*, partition::*};
use layer2_core::sandbox::agent_io::*;
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::Fixture;
use super::*;

pub(super) struct ChannelFixture {
    pub f: Fixture,
    pub grant: BridgeInvokeGrantView,
    pub inbox: StoredInbox,
    pub allocation: AllocationRecord,
    pub commands: Arc<MongoChannelCommandStore>,
    pub inbox_store: crate::store::MongoCallbackInbox,
}

impl ChannelFixture {
    pub async fn new() -> Self {
        Self::from_fixture(Fixture::new().await).await
    }
    pub async fn from_fixture(f: Fixture) -> Self {
        Self::from_source(f, false).await
    }
    pub async fn discord_text() -> Self {
        Self::from_source(Fixture::discord_text().await, true).await
    }
    async fn from_source(mut f: Fixture, text_attachment: bool) -> Self {
        let grant = f
            .service
            .issue(&f.context, Some("fixture-credential"), f.issue("consent"))
            .await
            .unwrap()
            .result;
        f.enable("enable").await;
        let validation = f
            .authority
            .validate_connection(&f.bridge.owner, &f.bridge.current().unwrap().config)
            .await
            .unwrap();
        let write = if text_attachment {
            let mut fixture = crate::bridge::attachments::discord_tests::fixture();
            fixture.bridge = f.bridge.clone();
            let config = &f.bridge.current().unwrap().config;
            fixture.body["agent"]["api_key_id"] = json!(config.connector.key_id);
            fixture.body["conversation"]["id"] = json!(config.connector.route_ids[0]);
            fixture
                .signed()
                .unwrap()
                .classify_capabilities(&fixture.bytes(), &crate::bridge::ingress::NyxidCapabilities)
                .unwrap()
                .bind(CallbackBinding::new(f.bridge.clone(), validation).unwrap())
                .unwrap()
        } else {
            signed_inbox(&f.bridge, validation).await
        };
        let inbox = write.record().clone();
        let key =
            crate::store::AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
        let inbox_store = crate::store::MongoCallbackInbox::open(
            &f.bridges,
            &f.database,
            "fixture-deployment",
            &key,
        )
        .await
        .unwrap();
        inbox_store.accept(write).await.unwrap();
        let source = NormalizedPartitionSource::from_stored(&inbox, &f.bridge).unwrap();
        let admission = AllocationAdmission::new(f.bridge.clone(), &source).unwrap();
        let proof = f
            .service
            .authorize(f.request(
                &grant,
                BridgeInvokeEffect::BindExistingTarget {
                    allocation_id: admission.identity().id().unwrap(),
                },
            ))
            .await
            .unwrap();
        let store = crate::store::MongoBridgeAllocationStore::open(
            f.bridges.clone(),
            Some(f.store.clone()),
            &f.database,
        )
        .await
        .unwrap();
        let allocation = store
            .reserve(
                AllocationReservation::new(
                    admission,
                    AllocationAuthority::Existing(Box::new(proof)),
                    now() * 1000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let commands = Arc::new(MongoChannelCommandStore::new(f.store.clone()));
        Self {
            f,
            grant,
            inbox,
            allocation,
            commands,
            inbox_store,
        }
    }
    pub async fn request(&self) -> ChannelCommandReservation {
        let proof = self
            .f
            .service
            .authorize(self.f.request(
                &self.grant,
                BridgeInvokeEffect::AdmitMessage {
                    inbox_id: self.inbox.id.clone(),
                },
            ))
            .await
            .unwrap();
        ChannelCommandReservation::prepare(proof, self.inbox.clone(), self.allocation.clone())
            .unwrap()
    }
    pub async fn close(self) {
        self.f.close().await;
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub(crate) async fn signed_inbox(
    bridge: &layer2_core::bridge::BridgeRecord,
    proof: layer2_core::bridge::BridgeValidation,
) -> InboxWrite {
    signed_inbox_named(bridge, proof, "external-message", "callback-1", 42).await
}

pub(crate) async fn signed_inbox_named(
    bridge: &layer2_core::bridge::BridgeRecord,
    proof: layer2_core::bridge::BridgeValidation,
    message_id: &str,
    callback_id: &str,
    platform_message_id: i64,
) -> InboxWrite {
    let config = &bridge.current().unwrap().config;
    let body = serde_json::to_vec(&json!({
        "message_id":message_id, "correlation_id":callback_id, "platform":"telegram",
        "agent":{"api_key_id":config.connector.key_id, "name":"Fixture"},
        "conversation":{"id":config.connector.route_ids[0], "platform_id":"-100123", "type":"private"},
        "sender":{"platform_id":"12345"}, "content":{"type":"text", "text":"Explain the result."},
        "timestamp":layer2_core::now_rfc3339(),
        "raw_platform_data":{"update_id":99, "message":{"message_id":platform_message_id, "date":1700000000,
            "chat":{"id":-100123,"type":"supergroup"},
            "from":{"id":12345,"is_bot":false}, "text":"Explain the result."}}
    }))
    .unwrap();
    let claims = json!({"iss":config.connector.issuer, "aud":"channel-relay/callback",
        "iat":now(), "exp":now()+300, "jti":callback_id, "token_type":"relay_callback",
        "api_key_id":config.connector.key_id, "message_id":message_id, "platform":"telegram",
        "body_sha256":format!("{:x}", Sha256::digest(&body))});
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
    let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let keys = CallbackKeySet::from_pinned_jwks(
        config.connector.issuer.clone(),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../layer2-core/src/bridge/ingress/fixtures/jwks.json"
        )),
        deadline,
    )
    .unwrap();
    let verifier = CallbackVerifier::new(CallbackTokenPolicy {
        issuer: config.connector.issuer.clone(),
        relay_audience: config.connector.issuer.clone(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    })
    .unwrap();
    verifier
        .preauthenticate(
            bridge.clone(),
            &keys,
            CallbackHeaders {
                callback_token: &token,
                user_token: None,
                message_id,
                platform: "telegram",
                timestamp: &layer2_core::now_rfc3339(),
            },
            &body,
        )
        .unwrap()
        .classify_capabilities(&body, &crate::bridge::ingress::NyxidCapabilities)
        .unwrap()
        .bind(CallbackBinding::new(bridge.clone(), proof).unwrap())
        .unwrap()
}

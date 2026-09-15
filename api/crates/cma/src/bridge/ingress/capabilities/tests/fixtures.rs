use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::resource_access::{ResourceActor, ResourceOwner};
use layer2_core::SubjectId;
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;

#[derive(Clone)]
pub(in crate::bridge::ingress) struct Fixture {
    pub bridge: BridgeRecord,
    pub body: Value,
}

impl Fixture {
    pub fn new(platform: BridgePlatform) -> Self {
        let name = serde_json::to_value(&platform).unwrap();
        let config: BridgeConfig = serde_json::from_value(json!({
            "display_name":"Disposable fixture",
            "connector":{"issuer":"https://nyx.example","base_url":"https://nyx.example",
                "bot_id":"nyx-bot-uuid","platform":name,"route_ids":["wildcard-route"],
                "key_id":"selected-key","custody":"brsec_00000000000000000000000000000001",
                "callback_url":"https://cma.example/callback/brg_fixture"},
            "partition":"conversation_sender",
            "placement":{"kind":"new_sandbox",
                "sandbox_profile":{"id":"sbp-1","revision":1},
                "workspace_profile":{"id":"wkp-1","revision":1},
                "agent_profile":{"id":"agp-1","revision":1}},
            "admission":{"kind":"scoped","policy":{"senders":{"kind":"open"},"chats":{"kind":"open"},"groups":"all"}},
            "reply":"final_response",
            "limits":{"max_partitions":5,"max_sandboxes":5,"max_queued_messages":10,
                "max_messages_per_minute":60,"max_payload_bytes":1048576,
                "inbox_retention_seconds":3600,"reply_secret_retention_seconds":1200}
        })).unwrap();
        let owner = ResourceOwner::Person {
            subject: SubjectId::new("fixture-owner"),
        };
        let actor = ResourceActor::Human {
            subject: SubjectId::new("fixture-owner"),
        };
        let time = layer2_core::now_rfc3339();
        let bridge = BridgeRecord {
            id: BridgeId("brg_fixture".into()),
            owner,
            revision: 1,
            config_revision: 1,
            state: BridgeState::Enabled,
            emergency: Default::default(),
            generations: vec![BridgeGeneration {
                config_revision: 1,
                config,
                created_at: time.clone(),
                actor: actor.clone(),
                previous_work: None,
            }],
            created_at: time.clone(),
            created_by: actor,
            updated_at: time.clone(),
        };
        let (chat, sender, raw) = match platform {
            BridgePlatform::Telegram => (
                "-100123",
                "12345",
                json!({
                "update_id":99,"message":{"message_id":42,"date":1700000000,
                    "chat":{"id":-100123,"type":"supergroup"},
                    "from":{"id":12345,"is_bot":false,"first_name":"Human"},"text":"hello"}}),
            ),
            BridgePlatform::Lark | BridgePlatform::Feishu => (
                "oc_chat",
                "ou_sender",
                json!({
                "schema":"2.0","header":{"event_type":"im.message.receive_v1","event_id":"event-1","app_id":"cli_app","token":"fixture-header-secret"},
                "event":{"sender":{"sender_id":{"open_id":"ou_sender"},"sender_type":"user"},
                    "message":{"message_id":"om_message","chat_id":"oc_chat","chat_type":"group",
                        "message_type":"text","content":"{\"text\":\"hello\"}"}}}),
            ),
            BridgePlatform::Discord => (
                "discord-chat",
                "discord-sender",
                json!({
                "op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"discord-message","type":0,
                    "channel_id":"discord-chat","guild_id":"guild-one",
                    "author":{"id":"discord-sender","bot":false},"content":"hello"}}),
            ),
        };
        let body = json!({"message_id":"nyx-message","correlation_id":"signed-callback",
            "platform":name,"agent":{"api_key_id":"selected-key","name":"Fixture"},
            "conversation":{"id":"wildcard-route","platform_id":chat,"type":"private"},
            "sender":{"platform_id":sender,"display_name":"Same display name"},
            "content":{"type":"text","text":"hello"},"timestamp":time,"raw_platform_data":raw});
        Self { bridge, body }
    }

    pub fn config(&self) -> &BridgeConfig {
        &self.bridge.generations[0].config
    }
    pub fn config_mut(&mut self) -> &mut BridgeConfig {
        &mut self.bridge.generations[0].config
    }
    pub fn raw_mut(&mut self) -> &mut Value {
        &mut self.body["raw_platform_data"]
    }
    pub fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.body).unwrap()
    }

    pub fn headers(&self) -> axum::http::HeaderMap {
        let body = self.bytes();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({"iss":"https://nyx.example","aud":"channel-relay/callback",
            "iat":now,"exp":now+300,"jti":self.body["correlation_id"],"token_type":"relay_callback",
            "api_key_id":"selected-key","message_id":self.body["message_id"],"platform":self.body["platform"],
            "body_sha256":format!("{:x}",Sha256::digest(&body))});
        let token = sign(&claims);
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in [
            ("content-type", "application/json"),
            (
                "x-nyxid-message-id",
                self.body["message_id"].as_str().unwrap(),
            ),
            ("x-nyxid-platform", self.body["platform"].as_str().unwrap()),
            ("x-nyxid-callback-token", token.as_str()),
        ] {
            headers.insert(name, value.parse().unwrap());
        }
        headers.insert(
            "x-nyxid-timestamp",
            layer2_core::now_rfc3339().parse().unwrap(),
        );
        headers
    }

    pub fn signed(&self) -> Result<SignedCallback, CallbackError> {
        let body = self.bytes();
        let headers = self.headers();
        let policy = CallbackTokenPolicy {
            issuer: "https://nyx.example".into(),
            relay_audience: "https://nyx.example".into(),
            callback_ttl_seconds: 300,
            relay_ttl_seconds: 300,
            reply_ttl_seconds: 1800,
            clock_skew_seconds: 60,
        };
        let keys = CallbackKeySet::from_pinned_jwks(
            policy.issuer.clone(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../layer2-core/src/bridge/ingress/fixtures/jwks.json"
            )),
            tokio::time::Instant::now() + Duration::from_secs(60),
        )
        .unwrap();
        CallbackVerifier::new(policy).unwrap().preauthenticate(
            self.bridge.clone(),
            &keys,
            CallbackHeaders {
                callback_token: headers["x-nyxid-callback-token"].to_str().unwrap(),
                user_token: None,
                message_id: self.body["message_id"].as_str().unwrap(),
                platform: self.body["platform"].as_str().unwrap(),
                timestamp: &layer2_core::now_rfc3339(),
            },
            &body,
        )
    }

    pub fn binding(&self) -> CallbackBinding {
        let owner = &self.bridge.owner;
        let config = self.config();
        let proof = BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 1,
                routes: vec![BridgeRouteObservation {
                    route_id: "wildcard-route".into(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                }],
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: None,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
        .unwrap();
        CallbackBinding::new(self.bridge.clone(), proof).unwrap()
    }

    pub fn write(&self) -> Result<InboxWrite, CallbackError> {
        self.signed()?
            .classify_capabilities(&self.bytes(), &NyxidCapabilities)?
            .bind(self.binding())
    }
    pub fn key(&self) -> NormalizedPartitionKey {
        let write = self.write().unwrap();
        match NormalizedPartitionSource::from_stored(write.record(), &self.bridge)
            .unwrap()
            .admit(self.config())
            .unwrap()
        {
            PartitionAdmissionDecision::Admitted { key, .. } => key,
            other => panic!("expected fixture admission: {other:?}"),
        }
    }
}

fn sign(claims: &Value) -> String {
    let key = RsaKeyPair::from_pkcs8(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../layer2-core/src/bridge/ingress/fixtures/callback-test-only.pk8"
    )))
    .unwrap();
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"alg":"RS256","typ":"JWT","kid":"fixture-key"})).unwrap()
        ),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
    );
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

pub(super) fn platforms() -> [BridgePlatform; 4] {
    [
        BridgePlatform::Telegram,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
        BridgePlatform::Discord,
    ]
}

pub(super) fn deny(fixture: &Fixture, reason: PartitionRefusal) {
    let write = fixture.write().unwrap();
    assert_eq!(
        write.record().admission,
        InboxAdmission::PartitionDenied { reason }
    );
    assert!(write.reply_token().is_none());
    assert!(write.thread_context().is_none());
    assert!(write.record().reply_expires_at.is_none());
    assert!(!write.record().envelope.protected_thread_context_present);
}

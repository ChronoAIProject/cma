use super::*;
use crate::bridge::ingress::reply_addressing::{ReplyCapabilities, ReplyLookup};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::bridge::outbox::*;
use ring::{
    rand::SystemRandom,
    signature::{RsaKeyPair, RSA_PKCS1_SHA256},
};
use sha2::{Digest, Sha256};

pub(crate) struct ControlFixture {
    pub f: ReplyFixture,
    pub invokes: Arc<crate::store::MongoBridgeInvokeStore>,
    pub store: crate::store::bridge_invokes::channel_controls::MongoChannelControlStore,
    pub callbacks: Arc<crate::store::MongoCallbackInbox>,
    pub reference: ChannelControlReferenceRecord,
    pub replied: OutboxItem,
}

impl ControlFixture {
    pub async fn new() -> Self {
        let f = ReplyFixture::new().await;
        let invokes = Arc::new(
            crate::store::MongoBridgeInvokeStore::open(
                f.bridges.clone(),
                f.resources.clone(),
                &f.database,
            )
            .await
            .unwrap(),
        );
        let bridge = f
            .bridges
            .bridge(&f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        BridgeSourceControlService::new(f.invokes.clone(), invokes.clone())
            .submit(
                &caller(),
                None,
                &f.grant.id,
                "control-consent",
                SourceControlRequest {
                    expected_bridge_revision: bridge.revision,
                    expected_grant_revision: 1,
                    expected_policy_revision: 0,
                    action: SourceControlAction::AllowSenderCancelAndSteer,
                },
            )
            .await
            .unwrap();
        let result = f.call("read_channel_control_context", json!({})).await;
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["structuredContent"]["operations"], json!([]));
        let reference = result["structuredContent"]["source_control_reference"]
            .as_str()
            .unwrap();
        let row = f
            .client
            .database(&f.database)
            .collection::<Document>("bridge_channel_control_references")
            .find_one(doc! {"_id":reference})
            .await
            .unwrap()
            .unwrap();
        let reference: ChannelControlReferenceRecord =
            crate::store::sandbox_mongo::read_record(&row).unwrap();
        let context = f.context().await;
        let result = f
            .reply(
                &context,
                "explicit-control-reference",
                "I am working on your request.",
            )
            .await;
        assert_eq!(result["isError"], false);
        let mut row = f.items().await.remove(0);
        let mut replied: OutboxItem = crate::store::sandbox_mongo::read_record(&row).unwrap();
        // A selected-bot Sent fixture derived from the actual explicit tool
        // call; no delivery producer or live provider is invoked by this test.
        replied.state = DeliveryState::Sent {
            receipt: DeliveryReceipt {
                nyxid_message_id: "control-pointer".into(),
                platform_message_id: Some("55".into()),
            },
        };
        replied.revision += 1;
        replied.validate().unwrap();
        row.insert("record", record_bson(&replied).unwrap());
        row.insert("revision", replied.revision as i64);
        row.insert("state", "sent");
        f.client
            .database(&f.database)
            .collection::<Document>("bridge_outbox_items")
            .replace_one(doc! {"_id":&replied.id}, row)
            .await
            .unwrap();
        let callbacks = Arc::new(
            crate::store::MongoCallbackInbox::open(
                &f.bridges,
                &f.database,
                "fixture-deployment",
                &custody_key(),
            )
            .await
            .unwrap(),
        );
        let store = crate::store::bridge_invokes::channel_controls::MongoChannelControlStore::open(
            invokes.clone(),
            callbacks.clone(),
        )
        .await
        .unwrap();
        Self {
            f,
            invokes,
            store,
            callbacks,
            reference,
            replied,
        }
    }

    pub async fn write(
        &self,
        callback: &str,
        provider: u64,
        sender: u64,
        reply: bool,
        steer: bool,
    ) -> ChannelControlWrite {
        self.write_with_lease(
            callback,
            provider,
            sender,
            reply,
            steer,
            Duration::from_secs(60),
        )
        .await
    }

    pub async fn write_with_lease(
        &self,
        callback: &str,
        provider: u64,
        sender: u64,
        reply: bool,
        steer: bool,
        key_lease: Duration,
    ) -> ChannelControlWrite {
        let text = if steer {
            format!(
                "CMA-CONTROL/1 steer {}\nKeep the current source and narrow the scope.",
                self.reference.reference
            )
        } else {
            format!("CMA-CONTROL/1 cancel {}", self.reference.reference)
        };
        let (body, _, _) =
            crate::store::bridge_definitions::mongo_integration_tests::inbox::callback_for_sender(
                callback, &text, sender,
            );
        let mut value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        value["correlation_id"] = json!(callback);
        value["raw_platform_data"]["message"]["message_id"] = json!(provider);
        if reply {
            value["reply_to_platform_message_id"] = json!("55");
            value["raw_platform_data"]["message"]["reply_to_message"] = json!({"message_id":55,"chat":{"id":-100123,"type":"supergroup"},"from":{"id":999,"is_bot":true}});
        }
        let body = serde_json::to_vec(&value).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let token = sign(
            json!({"iss":"https://nyx.example","aud":"channel-relay/callback","iat":now,"exp":now+300,
            "jti":callback,"token_type":"relay_callback","api_key_id":"key-1","message_id":callback,"platform":"telegram",
            "body_sha256":format!("{:x}",Sha256::digest(&body))}),
        );
        let bridge = self
            .f
            .bridges
            .bridge(&self.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
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
            tokio::time::Instant::now() + key_lease,
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
        let signed = verifier
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
        let confirmed = if reply {
            ReplyLookup::authenticated(&payload)
                .unwrap()
                .unwrap()
                .confirm(&self.replied)
                .unwrap()
        } else {
            None
        };
        let extractor = ReplyCapabilities(confirmed);
        let facts = extractor.observe(&payload).unwrap();
        let parent = if reply {
            VerifiedRedeliveryParent::Message("55".into())
        } else {
            VerifiedRedeliveryParent::Absent
        };
        let redelivery =
            VerifiedPlatformRedelivery::from_verified_observation(&payload, &facts, parent)
                .unwrap();
        let write = signed
            .classify_capabilities(&body, &extractor)
            .unwrap()
            .bind(binding)
            .unwrap()
            .with_platform_redelivery(redelivery)
            .unwrap();
        ChannelControlWrite::from_verified(write).unwrap()
    }

    pub async fn proof(
        &self,
        write: ChannelControlWrite,
    ) -> Result<ChannelControlProof, ChannelControlError> {
        let command = self
            .f
            .resources
            .get_command(&self.f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let row = self
            .f
            .client
            .database(&self.f.database)
            .collection::<Document>("agent_commands")
            .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let sidecar: ChannelCommandSidecar =
            mongodb::bson::deserialize_from_bson(row.get("channel_admission").unwrap().clone())
                .unwrap();
        let invoke = self
            .f
            .invokes
            .authorize(BridgeInvokeRequest {
                grant_id: self.f.grant.id.clone(),
                pin: self.f.inbox.pin.clone(),
                target: self.f.allocation.placement.target().clone(),
                effect: BridgeInvokeEffect::ExecuteCommand {
                    command_id: command.receipt.envelope.command_id.clone(),
                },
            })
            .await
            .unwrap();
        ChannelControlProof::from_current(
            write,
            invoke,
            ChannelControlObservation {
                reference: &self.reference,
                original_inbox: &self.f.inbox,
                source_command: &command,
                source_sidecar: &sidecar,
                allocation: &self.f.allocation,
                consent: &self.reference.binding().consent,
                consent_receipt: &self.reference.binding().consent_receipt,
                replied_item: Some(&self.replied),
            },
        )
    }

    pub async fn request(
        &self,
        callback: &str,
        provider: u64,
        steer: bool,
    ) -> ChannelControlReservation {
        ChannelControlReservation::prepare(
            self.proof(self.write(callback, provider, 12345, true, steer).await)
                .await
                .unwrap(),
        )
        .unwrap()
    }
}

fn sign(claims: serde_json::Value) -> String {
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

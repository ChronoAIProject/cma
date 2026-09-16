use super::*;
use crate::bridge::ingress::{
    controls::{ChannelControlIngress, ChannelControlRuntimeEligibility},
    reply_addressing::{ReplyCapabilities, ReplyLookup},
    CallbackIngress, PinnedCallbackKeys,
};
use crate::config::{bridge_flags::BridgeFlag, BridgeFeatureFlags};
use futures_util::TryStreamExt;
use layer2_core::sandbox::{AgentRuntimeRegistry, AgentRuntimeTarget};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

pub(super) struct Runtime {
    pub registry: Arc<crate::sandbox::LiveAgentRegistry>,
    pub calls: AtomicUsize,
}

impl Runtime {
    pub fn local(c: &ControlFixture) -> Arc<Self> {
        Arc::new(Self {
            registry: c.f.registry.clone(),
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl ChannelControlRuntimeEligibility for Runtime {
    async fn require_available(
        &self,
        proof: &ChannelControlProof,
    ) -> Result<(), ChannelControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        proof.require_current()?;
        let intent = proof.intent();
        let connection = AgentRuntimeRegistry::resolve(
            self.registry.as_ref(),
            &AgentRuntimeTarget {
                agent_id: intent.reference.binding().source.target().agent_id.clone(),
                runtime_generation: intent.runtime_generation,
                conversation_id: intent.runtime_conversation_id.clone(),
            },
        )
        .await
        .map_err(|_| ChannelControlError::Unavailable)?;
        let operation = match &intent.request.command {
            ChannelControlCommand::Cancel => layer2_core::RuntimeCapability::Interrupt,
            ChannelControlCommand::Steer { .. } => layer2_core::RuntimeCapability::Steer,
        };
        if !connection.capabilities.supports(operation) {
            return Err(ChannelControlError::CapabilityUnsupported);
        }
        Ok(())
    }
}

pub(super) fn flags(enabled: bool) -> BridgeFeatureFlags {
    BridgeFeatureFlags::from_lookup(&mut |key| {
        Ok((enabled && key == BridgeFlag::ControlAdmission.key()).then(|| "true".into()))
    })
    .unwrap()
}

pub(super) async fn app(
    c: &ControlFixture,
    enabled: bool,
    runtime: Option<Arc<dyn ChannelControlRuntimeEligibility>>,
) -> axum::Router {
    // Reopen both production stores for every router, exercising persistence
    // independently from the fixture's direct admission convenience methods.
    let callbacks = Arc::new(
        crate::store::MongoCallbackInbox::open(
            &c.f.bridges,
            &c.f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let store = Arc::new(
        crate::store::MongoChannelControlStore::open(c.invokes.clone(), callbacks.clone())
            .await
            .unwrap(),
    );
    let mut controls = ChannelControlIngress::production(
        callbacks.clone(),
        store,
        c.f.invokes.clone(),
        flags(enabled),
    );
    if let Some(runtime) = runtime {
        controls = controls.with_runtime(runtime);
    }
    let ingress = CallbackIngress::new(
        c.f.bridges.clone(),
        Arc::new(Authority),
        callbacks,
        PinnedCallbackKeys::fixture(),
        policy(),
        "https://cma.example/api/v1/bridge-callbacks".into(),
    )
    .unwrap()
    .with_reply_addressing(c.f.replies.clone())
    .with_channel_controls(Arc::new(controls));
    crate::bridge::ingress::http::router(Some(Arc::new(ingress)))
}

fn policy() -> CallbackTokenPolicy {
    CallbackTokenPolicy {
        issuer: "https://nyx.example".into(),
        relay_audience: "https://nyx.example".into(),
        callback_ttl_seconds: 300,
        relay_ttl_seconds: 300,
        reply_ttl_seconds: 1800,
        clock_skew_seconds: 60,
    }
}

pub(super) fn body(c: &ControlFixture, callback: &str, provider: u64, steer: bool) -> Value {
    let text = if steer {
        format!(
            "CMA-CONTROL/1 steer {}\nKeep this same turn focused.",
            c.reference.reference
        )
    } else {
        format!("CMA-CONTROL/1 cancel {}", c.reference.reference)
    };
    let (bytes, _, _) =
        crate::store::bridge_definitions::mongo_integration_tests::inbox::callback(callback, &text);
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    value.as_object_mut().unwrap().remove("reply_token");
    value.as_object_mut().unwrap().remove("thread_id");
    value["raw_platform_data"]
        .as_object_mut()
        .unwrap()
        .remove("secret");
    value["correlation_id"] = json!(callback);
    value["raw_platform_data"]["message"]["message_id"] = json!(provider);
    value["reply_to_platform_message_id"] = json!("55");
    value["raw_platform_data"]["message"]["reply_to_message"] = json!({"message_id":55,"chat":{"id":-100123,"type":"supergroup"},"from":{"id":999,"is_bot":true}});
    value
}

fn signed(value: &Value) -> (Vec<u8>, String) {
    let bytes = serde_json::to_vec(value).unwrap();
    let callback = value["message_id"].as_str().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let token = super::super::fixture::sign(
        json!({"iss":"https://nyx.example","aud":"channel-relay/callback",
        "iat":now,"exp":now+300,"jti":callback,"token_type":"relay_callback","api_key_id":"key-1",
        "message_id":callback,"platform":"telegram","body_sha256":format!("{:x}",Sha256::digest(&bytes))}),
    );
    (bytes, token)
}

pub(super) async fn post(
    app: axum::Router,
    c: &ControlFixture,
    value: &Value,
) -> (StatusCode, Value) {
    let (bytes, token) = signed(value);
    let response = app
        .oneshot(
            Request::post(format!(
                "/api/v1/bridge-callbacks/{}",
                c.f.inbox.pin.bridge_id.as_str()
            ))
            .header("content-type", "application/json")
            .header("x-nyxid-callback-token", token)
            .header("x-nyxid-message-id", value["message_id"].as_str().unwrap())
            .header("x-nyxid-platform", "telegram")
            .header("x-nyxid-timestamp", layer2_core::now_rfc3339())
            .body(Body::from(bytes))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    (status, value)
}

pub(super) async fn legacy_source(c: &ControlFixture, value: &Value) -> StoredInbox {
    let (bytes, token) = signed(value);
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
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
        tokio::time::Instant::now() + Duration::from_secs(60),
    )
    .unwrap();
    let signed = CallbackVerifier::new(policy())
        .unwrap()
        .preauthenticate(
            bridge,
            &keys,
            CallbackHeaders {
                callback_token: &token,
                user_token: None,
                message_id: value["message_id"].as_str().unwrap(),
                platform: "telegram",
                timestamp: &layer2_core::now_rfc3339(),
            },
            &bytes,
        )
        .unwrap();
    let payload = signed.authenticated_payload(&bytes).unwrap();
    let reply = ReplyLookup::authenticated(&payload)
        .unwrap()
        .unwrap()
        .confirm(&c.replied)
        .unwrap();
    signed
        .classify_capabilities(&bytes, &ReplyCapabilities(reply))
        .unwrap()
        .bind(binding)
        .unwrap()
        .record()
        .clone()
}

pub(super) async fn rows(c: &ControlFixture, name: &str) -> u64 {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>(name)
        .count_documents(doc! {})
        .await
        .unwrap()
}

pub(super) async fn no_message_effects(c: &ControlFixture, controls: u64) {
    assert_eq!(rows(c, "bridge_callback_inbox").await, 1);
    assert_eq!(rows(c, "bridge_outbox_items").await, 1);
    assert_eq!(rows(c, "agent_commands").await, 1 + controls);
    assert_eq!(rows(c, "bridge_channel_control_intents").await, controls);
    let rows =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .find(doc! {"channel_control_admission":{"$exists":true}})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
    for row in rows {
        assert!(!row.contains_key("channel_admission"));
        assert!(!row.contains_key("channel_source"));
        let admitted = AdmittedChannelControl {
            command: crate::store::agent_commands::read_command(&row).unwrap(),
            sidecar: mongodb::bson::deserialize_from_bson(
                row.get("channel_control_admission").unwrap().clone(),
            )
            .unwrap(),
        };
        admitted.validate().unwrap();
        assert_eq!(
            admitted.sidecar.execution_hold,
            ChannelControlHold::WorkerUnavailable
        );
        assert_eq!(admitted.command.work, CommandWorkState::default());
        assert_eq!(admitted.command.status, AgentResponseStatus::WaitingAction);
        assert!(admitted.command.receipt.response_id.is_none());
    }
    let source =
        c.f.resources
            .get_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(source, c.f.command);
    let session = c.f.tools.authenticate(&c.f.credential).await.unwrap();
    assert_eq!(
        c.f.replies.control_context(&session).await.unwrap(),
        Some(c.reference.reference.clone())
    );
}

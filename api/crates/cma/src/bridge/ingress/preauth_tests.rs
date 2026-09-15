use std::sync::atomic::{AtomicUsize, Ordering};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer2_core::bridge::ingress::InboxWrite;
use layer2_core::bridge::*;
use layer2_core::profiles::*;
use layer2_core::resource_access::{ResourceActor, ResourceOwner};
use layer2_core::SubjectId;
use serde_json::json;

use super::*;

struct Fixture {
    record: BridgeRecord,
    provider_calls: AtomicUsize,
    inbox_calls: AtomicUsize,
}
#[async_trait::async_trait]
impl BridgeStore for Fixture {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        Ok((id == &self.record.id).then(|| self.record.clone()))
    }
    async fn receipt(&self, _: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        unreachable!()
    }
    async fn list(
        &self,
        _: &ResourceOwner,
        _: &str,
        _: Option<&BridgeId>,
        _: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        unreachable!()
    }
    async fn apply(&self, _: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Fixture {
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.provider_calls.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::AuthorityUnavailable)
    }
}
#[async_trait::async_trait]
impl CallbackInboxStore for Fixture {
    async fn accept(&self, _: InboxWrite) -> Result<InboxReceipt, CallbackError> {
        self.inbox_calls.fetch_add(1, Ordering::SeqCst);
        Err(CallbackError::Unavailable)
    }
}
fn fixture() -> Arc<Fixture> {
    let actor = ResourceActor::Human {
        subject: SubjectId::new("owner"),
    };
    let time = "2026-09-15T00:00:00Z".to_string();
    let config = BridgeConfig {
        display_name: "Fixture".into(),
        connector: BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route".into()],
            key_id: "key".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/callback/brg_fixture".into(),
        },
        partition: BridgePartitionPolicy::Conversation,
        placement: BridgePlacementPolicy::NewSandbox {
            sandbox_profile: SandboxProfileRef::new(
                layer2_core::SandboxProfileId("sbp-1".into()),
                1,
            ),
            workspace_profile: WorkspaceProfileRef::new(
                layer2_core::WorkspaceProfileId("wkp-1".into()),
                1,
            ),
            agent_profile: AgentProfileRef::new(layer2_core::AgentProfileId("agp-1".into()), 1),
        },
        admission: BridgeAdmissionPolicy::All,
        reply: BridgeReplyPolicy::Disabled,
        limits: BridgeLimits {
            max_partitions: 2,
            max_sandboxes: 2,
            max_queued_messages: 5,
            max_messages_per_minute: 60,
            max_payload_bytes: 4096,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 600,
        },
    };
    let record = BridgeRecord {
        id: BridgeId("brg_fixture".into()),
        owner: ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        revision: 2,
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
        updated_at: time,
    };
    record.validate().unwrap();
    Arc::new(Fixture {
        record,
        provider_calls: AtomicUsize::new(0),
        inbox_calls: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn invalid_attestation_has_zero_live_provider_custody_or_inbox_calls() {
    let fixture = fixture();
    let service = CallbackIngress::new(
        fixture.clone(),
        fixture.clone(),
        fixture.clone(),
        PinnedCallbackKeys::fixture(),
        CallbackTokenPolicy {
            issuer: "https://nyx.example".into(),
            relay_audience: "https://nyx.example".into(),
            callback_ttl_seconds: 300,
            relay_ttl_seconds: 300,
            reply_ttl_seconds: 1800,
            clock_skew_seconds: 60,
        },
        "https://cma.example/callback".into(),
    )
    .unwrap();
    let token = format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"alg":"RS256","typ":"JWT","kid":"fixture-key"})).unwrap()
        ),
        URL_SAFE_NO_PAD.encode(b"{}"),
        URL_SAFE_NO_PAD.encode([0_u8; 256])
    );
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("content-type", "application/json"),
        ("x-nyxid-message-id", "message"),
        ("x-nyxid-platform", "telegram"),
    ] {
        headers.insert(name, value.parse().unwrap());
    }
    headers.insert(
        "x-nyxid-timestamp",
        layer2_core::now_rfc3339().parse().unwrap(),
    );
    headers.insert("x-nyxid-callback-token", token.parse().unwrap());
    assert_eq!(
        service.accept(&fixture.record.id, &headers, b"{}").await,
        Err(CallbackError::Authentication)
    );
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.inbox_calls.load(Ordering::SeqCst), 0);
}

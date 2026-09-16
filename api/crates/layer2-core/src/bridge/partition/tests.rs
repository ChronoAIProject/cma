//! Synthetic verified observations exercise domain contracts; these fixtures
//! are not evidence that a live platform adapter supplies those capabilities.
use crate::bridge::ingress::{
    CallbackAttestation, InboxAdmission, InboxEnvelope, RelayCorroboration, StoredInbox,
};
use crate::bridge::*;
use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};

use super::*;

mod admission_tests;
mod identity_tests;

const AT: &str = "2026-09-15T06:00:00Z";

#[derive(Clone)]
struct Fixture {
    bridge: BridgeRecord,
    inbox: StoredInbox,
}

impl Fixture {
    fn new(platform: BridgePlatform, partition: BridgePartitionPolicy) -> Self {
        let owner = ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        };
        let actor = ResourceActor::Human {
            subject: SubjectId::new("owner"),
        };
        let config = BridgeConfig {
            display_name: "Fixture bridge".into(),
            connector: BridgeConnector {
                issuer: "https://nyx.example".into(),
                base_url: "https://nyx.example".into(),
                bot_id: "bot-one".into(),
                platform: platform.clone(),
                route_ids: vec!["wildcard-route".into(), "another-route".into()],
                key_id: "key-one".into(),
                custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into())
                    .unwrap(),
                callback_url: "https://cma.example/bridge/fixture".into(),
            },
            partition,
            placement: if partition == BridgePartitionPolicy::FixedSharedAgent {
                BridgePlacementPolicy::ExistingAgent {
                    target: BridgeAgentTarget {
                        sandbox_id: SandboxId("sbx_one".into()),
                        workspace_id: WorkspaceId("wks_one".into()),
                        worktree_id: WorktreeId("wkt_one".into()),
                        agent_id: AgentId("agt_one".into()),
                    },
                }
            } else {
                BridgePlacementPolicy::NewSandbox {
                    sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_one".into()), 1),
                    workspace_profile: WorkspaceProfileRef::new(
                        WorkspaceProfileId("wkp_one".into()),
                        1,
                    ),
                    agent_profile: AgentProfileRef::new(AgentProfileId("agp_one".into()), 1),
                }
            },
            admission: BridgeAdmissionPolicy::Scoped {
                policy: ChannelAdmissionPolicy {
                    senders: IdentityAdmission::Open,
                    chats: IdentityAdmission::Open,
                    groups: GroupAdmission::All,
                },
            },
            reply: BridgeReplyPolicy::FinalResponse,
            limits: BridgeLimits {
                max_partitions: 100,
                max_sandboxes: 100,
                max_queued_messages: 100,
                max_messages_per_minute: 60,
                max_payload_bytes: 4096,
                inbox_retention_seconds: 3600,
                reply_secret_retention_seconds: 600,
            },
        };
        let bridge = BridgeRecord {
            id: BridgeId("brg_fixture".into()),
            owner: owner.clone(),
            revision: 1,
            config_revision: 1,
            state: BridgeState::Enabled,
            emergency: BridgeEmergencyControls::default(),
            generations: vec![BridgeGeneration {
                config_revision: 1,
                config,
                created_at: AT.into(),
                actor: actor.clone(),
                previous_work: None,
            }],
            created_at: AT.into(),
            created_by: actor,
            updated_at: AT.into(),
        };
        let inbox = StoredInbox {
            id: "1".repeat(64),
            owner,
            pin: bridge.admission_pin().unwrap(),
            bot_id: "bot-one".into(),
            key_id: "key-one".into(),
            message_id: "message-one".into(),
            correlation_id: "correlation-one".into(),
            envelope: InboxEnvelope {
                platform: serde_json::from_value(serde_json::to_value(platform).unwrap()).unwrap(),
                route_id: "wildcard-route".into(),
                chat_id: "chat-one".into(),
                conversation_type: "group".into(),
                sender_id: "sender-one".into(),
                sender_display_name: Some("Display name".into()),
                content_type: "text".into(),
                text: Some("Keep this out of the key".into()),
                attachments: vec![],
                reply_to_message_id: None,
                reply_to_platform_message_id: None,
                protected_thread_context_present: false,
                timestamp: AT.into(),
            },
            attestation: CallbackAttestation {
                issuer: "https://nyx.example".into(),
                signing_key_id: "signing-one".into(),
                token_id: "token-one".into(),
                issued_at: 100,
                expires_at: 200,
                body_sha256: "2".repeat(64),
            },
            relay: RelayCorroboration::AbsentInboxOnly,
            admission: InboxAdmission::Accepted,
            capabilities: None,
            execution_hold: None,
            accepted_at: 101,
            expires_at: 3701,
            reply_expires_at: Some(701),
        };
        Self { bridge, inbox }
    }

    fn config(&self) -> &BridgeConfig {
        &self.bridge.generations.last().unwrap().config
    }
    fn config_mut(&mut self) -> &mut BridgeConfig {
        &mut self.bridge.generations.last_mut().unwrap().config
    }
    fn facts(&self) -> PlatformCapabilityObservation {
        let mut facts = PlatformCapabilityObservation::unknown(&self.inbox).unwrap();
        facts.sender_kind = ChannelSenderKind::Human;
        facts.event_kind = ChannelEventKind::UserMessage;
        facts.loop_evidence = LoopEvidence::NotLoop;
        facts.mentioned_bot = VerifiedFlag::No;
        facts.replied_to_bot = VerifiedFlag::No;
        facts.thread = ThreadEvidence::Missing;
        facts
    }
    fn source(&self, facts: PlatformCapabilityObservation) -> NormalizedPartitionSource {
        NormalizedPartitionSource::from_observation(&self.inbox, &self.bridge, facts).unwrap()
    }
    fn decision(&self, facts: PlatformCapabilityObservation) -> PartitionAdmissionDecision {
        self.source(facts).admit(self.config()).unwrap()
    }
    fn key(&self) -> NormalizedPartitionKey {
        self.key_with(self.facts())
    }
    fn key_with(&self, facts: PlatformCapabilityObservation) -> NormalizedPartitionKey {
        match self.decision(facts) {
            PartitionAdmissionDecision::Admitted { key, .. } => key,
            refused => panic!("expected admitted fixture: {refused:?}"),
        }
    }
}

fn platforms() -> [BridgePlatform; 5] {
    [
        BridgePlatform::Telegram,
        BridgePlatform::TelegramNew,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
    ]
}

fn stable_thread(platform: &BridgePlatform, id: &str) -> ThreadEvidence {
    let kind = match platform {
        BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
            StableThreadKind::TelegramForumTopic
        }
        BridgePlatform::Discord => StableThreadKind::DiscordChannelThread,
        BridgePlatform::Lark => StableThreadKind::LarkThread,
        BridgePlatform::Feishu => StableThreadKind::FeishuThread,
        BridgePlatform::Slack => StableThreadKind::SlackThread,
        BridgePlatform::Whatsapp | BridgePlatform::X => return ThreadEvidence::Unsupported,
    };
    ThreadEvidence::Verified(StableThreadReference::from_verified_field(kind, id.into()).unwrap())
}

fn refused(reason: PartitionRefusal) -> PartitionAdmissionDecision {
    PartitionAdmissionDecision::Refused { reason }
}

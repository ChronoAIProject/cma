use super::*;

pub(super) fn grant() -> BridgeInvokeGrant {
    let owner = ResourceOwner::Person {
        subject: SubjectId::new("owner"),
    };
    let actor = ResourceActor::Human {
        subject: SubjectId::new("owner"),
    };
    let target = BridgeInvokeTarget {
        ids: BridgeAgentTarget {
            sandbox_id: SandboxId("sandbox".into()),
            workspace_id: WorkspaceId("workspace".into()),
            worktree_id: WorktreeId("worktree".into()),
            agent_id: AgentId("agent".into()),
        },
        sandbox_created_at: "2026-09-15T00:00:00Z".into(),
        workspace_created_at: "2026-09-15T00:00:00Z".into(),
        agent_created_at: "2026-09-15T00:00:00Z".into(),
        agent_creation_fingerprint: "a".repeat(64),
        worktree_identity_digest: "b".repeat(64),
    };
    let config = BridgeConfig {
        display_name: "Shared retained fixture".into(),
        connector: BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route".into()],
            key_id: "key".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/bridge".into(),
        },
        partition: BridgePartitionPolicy::Conversation,
        placement: BridgePlacementPolicy::ExistingAgent {
            target: target.ids.clone(),
        },
        admission: BridgeAdmissionPolicy::All,
        reply: BridgeReplyPolicy::FinalResponse,
        limits: BridgeLimits {
            max_partitions: 2,
            max_sandboxes: 2,
            max_queued_messages: 5,
            max_messages_per_minute: 30,
            max_payload_bytes: 4096,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 600,
        },
    };
    let resource_audit = [
        ResourceCapability::ManageBridge,
        ResourceCapability::Interact,
    ]
    .into_iter()
    .map(|capability| ResourceAuthorizationAudit {
        target: target.resource(),
        sandbox_id: target.ids.sandbox_id.clone(),
        sandbox_created_at: target.sandbox_created_at.clone(),
        owner: owner.clone(),
        actor: actor.clone(),
        policy_revision: 0,
        capability,
        captured_at: "2026-09-15T00:00:00Z".into(),
        observations: vec![ResourceAuthorityObservation {
            capabilities: vec![capability],
            basis: ResourceAuthorizationBasis::PersonalOwner,
        }],
    })
    .collect();
    BridgeInvokeGrant {
        id: BridgeInvokeGrantId(format!("brinv_{}", "c".repeat(64))),
        revision: 1,
        owner,
        pin: BridgeWorkPin {
            bridge_id: BridgeId("brg_fixture".into()),
            config_revision: 1,
        },
        config,
        target,
        consent: BridgeSharedContextConsent::UseRetainedAgentContext,
        issued_at: "2026-09-15T00:00:00Z".into(),
        issued_by: actor,
        resource_audit,
        state: BridgeInvokeGrantState::Active,
    }
}
pub(super) fn bridge(grant: &BridgeInvokeGrant) -> BridgeRecord {
    BridgeRecord {
        id: grant.pin.bridge_id.clone(),
        owner: grant.owner.clone(),
        revision: 2,
        config_revision: 1,
        state: BridgeState::Enabled,
        emergency: Default::default(),
        generations: vec![BridgeGeneration {
            config_revision: 1,
            config: grant.config.clone(),
            created_at: grant.issued_at.clone(),
            actor: grant.issued_by.clone(),
            previous_work: None,
        }],
        created_at: grant.issued_at.clone(),
        created_by: grant.issued_by.clone(),
        updated_at: grant.issued_at.clone(),
    }
}

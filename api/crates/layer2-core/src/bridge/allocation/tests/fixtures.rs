use std::time::Duration;

use crate::bridge::{ingress::*, partition::*, *};
use crate::profiles::*;
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::SubjectId;

use super::super::*;

pub(super) fn fixture() -> (AllocationAdmission, AllocationAuthority) {
    let owner = ResourceOwner::Person {
        subject: SubjectId::new("owner"),
    };
    let actor = ResourceActor::Human {
        subject: SubjectId::new("owner"),
    };
    let config: BridgeConfig = serde_json::from_value(serde_json::json!({
        "display_name":"Allocation fixture",
        "connector":{"issuer":"https://nyx.example","base_url":"https://nyx.example",
            "bot_id":"bot-one","platform":"telegram","route_ids":["route-one"],
            "key_id":"key-one","custody":"brsec_00000000000000000000000000000001",
            "callback_url":"https://cma.example/callback"},
        "partition":"conversation_sender",
        "placement":{"kind":"new_sandbox","sandbox_profile":{"id":"sbp_one","revision":1},
            "workspace_profile":{"id":"wkp_one","revision":1},"agent_profile":{"id":"agp_one","revision":1}},
        "admission":{"kind":"all"},"reply":"final_response",
        "limits":{"max_partitions":10,"max_sandboxes":10,"max_queued_messages":20,
            "max_messages_per_minute":60,"max_payload_bytes":4096,
            "inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}
    })).unwrap();
    let at = "2026-09-15T06:00:00Z";
    let bridge = BridgeRecord {
        id: BridgeId("brg_fixture".into()),
        owner: owner.clone(),
        revision: 1,
        config_revision: 1,
        state: BridgeState::Enabled,
        emergency: BridgeEmergencyControls::default(),
        generations: vec![BridgeGeneration {
            config_revision: 1,
            config: config.clone(),
            created_at: at.into(),
            actor: actor.clone(),
            previous_work: None,
        }],
        created_at: at.into(),
        created_by: actor.clone(),
        updated_at: at.into(),
    };
    let inbox = StoredInbox {
        id: "a".repeat(64),
        owner: owner.clone(),
        pin: bridge.admission_pin().unwrap(),
        bot_id: "bot-one".into(),
        key_id: "key-one".into(),
        message_id: "message-one".into(),
        correlation_id: "correlation-one".into(),
        envelope: InboxEnvelope {
            platform: "telegram".into(),
            route_id: "route-one".into(),
            chat_id: "chat-one".into(),
            conversation_type: "private".into(),
            sender_id: "sender-one".into(),
            sender_display_name: None,
            content_type: "text".into(),
            text: Some("fixture".into()),
            attachments: vec![],
            reply_to_message_id: None,
            reply_to_platform_message_id: None,
            protected_thread_context_present: false,
            timestamp: at.into(),
        },
        attestation: CallbackAttestation {
            issuer: "https://nyx.example".into(),
            signing_key_id: "signing-one".into(),
            token_id: "token-one".into(),
            issued_at: 100,
            expires_at: 200,
            body_sha256: "b".repeat(64),
        },
        relay: RelayCorroboration::AbsentInboxOnly,
        admission: InboxAdmission::Accepted,
        capabilities: None,
        execution_hold: None,
        accepted_at: 101,
        expires_at: 3601,
        reply_expires_at: None,
    };
    let mut observation = PlatformCapabilityObservation::unknown(&inbox).unwrap();
    observation.sender_kind = ChannelSenderKind::Human;
    observation.event_kind = ChannelEventKind::UserMessage;
    observation.loop_evidence = LoopEvidence::NotLoop;
    let source = NormalizedPartitionSource::from_observation(&inbox, &bridge, observation).unwrap();
    let admission = AllocationAdmission::new(bridge.clone(), &source).unwrap();
    let sandbox_content: SandboxProfileContent = serde_json::from_value(serde_json::json!({
        "cpu_millis":1000,"memory_mib":1024,"agent_provider":"codex",
        "model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1
    }))
    .unwrap();
    let workspace_content: WorkspaceProfileContent = serde_json::from_value(serde_json::json!({
        "location":{"type":"directory","name":"fixture"},"name":"fixture"
    }))
    .unwrap();
    let agent_content: AgentProfileContent = serde_json::from_value(serde_json::json!({
        "thinking_effort":"low","permission_level":"guarded"
    }))
    .unwrap();
    let sandbox = SandboxProfileSnapshot {
        id: crate::SandboxProfileId("sbp_one".into()),
        revision: 1,
        digest: content_digest(&sandbox_content).unwrap(),
        content: sandbox_content,
    };
    let workspace = WorkspaceProfileSnapshot {
        id: crate::WorkspaceProfileId("wkp_one".into()),
        revision: 1,
        digest: content_digest(&workspace_content).unwrap(),
        content: workspace_content,
    };
    let agent = AgentProfileSnapshot {
        id: crate::AgentProfileId("agp_one".into()),
        revision: 1,
        digest: content_digest(&agent_content).unwrap(),
        content: agent_content,
    };
    let profiles = AllocationProfiles {
        trust_digest: AllocationProfiles::digest(&sandbox, &workspace, &agent).unwrap(),
        sandbox,
        workspace,
        agent,
        approved_by: actor,
    };
    let plan = AllocationProfilePlan {
        profiles,
        bridge: bridge.clone(),
        pin: bridge.admission_pin().unwrap(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(15),
    };
    let connection = BridgeValidation::from_observation(
        &owner,
        &config,
        BridgeConnectionObservation {
            connector: config.connector.clone(),
            bot_owner: owner.clone(),
            key_owner: owner.clone(),
            routes: vec![BridgeRouteObservation {
                route_id: "route-one".into(),
                owner: owner.clone(),
                bot_id: "bot-one".into(),
                key_id: "key-one".into(),
                enabled: true,
            }],
            custody_owner: owner.clone(),
            custody_revision: 1,
            key_state_version: 3,
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
    let authority = AllocationAuthority::Provisioning(Box::new(
        AllocationProvisioningProof::new(bridge, admission.identity(), plan, connection).unwrap(),
    ));
    (admission, authority)
}

pub(super) fn reserved() -> (AllocationRecord, AllocationAuthority) {
    let (admission, authority) = fixture();
    let write = AllocationReservation::new(admission, authority.clone(), 1000).unwrap();
    (write.record().clone(), authority)
}

pub(super) fn observation(record: &AllocationRecord) -> AllocationResourceObservation {
    AllocationResourceObservation {
        owner: record.owner.clone(),
        operation_id: record.operation_id.clone(),
        target: record.placement.target().clone(),
        receipt_digest: "c".repeat(64),
        profiles_digest: record.profiles_digest().map(str::to_owned),
    }
}

pub(super) fn renewed(
    bridge: BridgeRecord,
    record: &AllocationRecord,
) -> Result<AllocationAuthority, AllocationError> {
    let AllocationPlacement::NewSandbox { profiles, .. } = &record.placement else {
        unreachable!()
    };
    let config = &bridge.pinned(&record.pin).unwrap().config;
    let owner = &bridge.owner;
    let connection = BridgeValidation::from_observation(
        owner,
        config,
        BridgeConnectionObservation {
            connector: config.connector.clone(),
            bot_owner: owner.clone(),
            key_owner: owner.clone(),
            routes: vec![BridgeRouteObservation {
                route_id: "route-one".into(),
                owner: owner.clone(),
                bot_id: "bot-one".into(),
                key_id: "key-one".into(),
                enabled: true,
            }],
            custody_owner: owner.clone(),
            custody_revision: 1,
            key_state_version: 3,
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
    let plan = AllocationProfilePlan {
        profiles: profiles.as_ref().clone(),
        bridge: bridge.clone(),
        pin: record.pin.clone(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(15),
    };
    AllocationProvisioningProof::new(bridge, &record.identity, plan, connection)
        .map(|proof| AllocationAuthority::Provisioning(Box::new(proof)))
}

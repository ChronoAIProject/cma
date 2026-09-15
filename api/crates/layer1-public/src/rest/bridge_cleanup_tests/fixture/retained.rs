use super::*;
use layer2_core::{
    profiles::content_digest,
    sandbox::{AgentStatus, Sandbox, Workspace},
};
use serde_json::json;

pub(super) fn config(sandbox: &Sandbox, workspace: &Workspace) -> BridgeConfig {
    serde_json::from_value(json!({
        "display_name":"Cleanup retained fixture","connector":{"issuer":"https://identity.test","base_url":"https://identity.test","bot_id":"bot","platform":"telegram",
        "route_ids":["route"],"key_id":"key","custody":"brsec_00000000000000000000000000000001","callback_url":"https://cma.example/bridge"},
        "partition":"conversation_sender","placement":{"kind":"worktree_agent_per_partition","sandbox_id":sandbox.id,"workspace_id":workspace.id,
        "agent_profile":{"id":"agp_route","revision":1},"shared_trust":"all_participants_share_filesystem_processes_and_credentials",
        "capacity":{"keep_other_workspaces":1,"keep_other_agents_per_workspace":1}},"admission":{"kind":"all"},"reply":"disabled",
        "limits":{"max_partitions":4,"max_sandboxes":4,"max_queued_messages":10,"max_messages_per_minute":30,"max_payload_bytes":4096,"inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}
    })).unwrap()
}

pub(super) fn allocation(
    bridge: BridgeId,
    config: BridgeConfig,
    sandbox: &Sandbox,
    workspace: &Workspace,
) -> AllocationRecord {
    let identity = AllocationIdentity {
        bridge_id: bridge.clone(),
        key: serde_json::from_value(json!({"version":1,"hash":"1".repeat(64)})).unwrap(),
        generation: 1,
    };
    let id = identity.id().unwrap();
    let suffix =
        |kind| content_digest(&("cma.bridge.resource.v1", &id, kind)).unwrap()[..32].to_owned();
    let target = BridgeAgentTarget {
        sandbox_id: sandbox.id.clone(),
        workspace_id: workspace.id.clone(),
        worktree_id: layer2_core::WorktreeId(format!("wkt_{}", suffix("worktree"))),
        agent_id: layer2_core::AgentId(format!("agt_{}", suffix("agent"))),
    };
    let mut agent =
        agents_tests::seeded_agent("unused-history", AgentStatus::Ready, 0, None).profile;
    agent.digest = content_digest(&agent.content).unwrap();
    let profiles = AllocationProfiles {
        trust_digest: AllocationProfiles::digest(&sandbox.profile, &workspace.profile, &agent)
            .unwrap(),
        sandbox: sandbox.profile.clone(),
        workspace: workspace.profile.clone(),
        agent,
        approved_by: context("owner-a").actor,
    };
    let pin = BridgeWorkPin {
        bridge_id: bridge,
        config_revision: 1,
    };
    let operation_id = format!("{id}:provision:v1");
    let record = AllocationRecord {
        id,
        identity,
        owner: ResourceOwner::Person {
            subject: sandbox.owner.clone(),
        },
        pin: pin.clone(),
        config,
        first_source: serde_json::from_value(
            json!({"inbox_id":"2".repeat(64),"body_sha256":"3".repeat(64),"work_pin":pin}),
        )
        .unwrap(),
        operation_id: operation_id.clone(),
        predecessor: None,
        state: AllocationState::Retired,
        revision: 4,
        lease_fence: 1,
        lease: None,
        effect_started: true,
        receipt: Some(AllocationResourceReceipt {
            operation_id,
            target: target.clone(),
            receipt_digest: "4".repeat(64),
            profiles_digest: Some(profiles.trust_digest.clone()),
        }),
        placement: AllocationPlacement::ReuseParent {
            target,
            profiles: Box::new(profiles),
            parent: AllocationParentIdentity {
                sandbox_created_at: sandbox.created_at.clone(),
                workspace_created_at: Some(workspace.created_at.clone()),
                limits: sandbox.limits,
                service_scope: sandbox.credential.effective_reaches(),
            },
        },
        created_at_ms: 0,
        updated_at_ms: 500,
    };
    record.validate().unwrap();
    record
}

pub(super) fn status(record: &AllocationRecord) -> ChildCleanupStatus {
    ChildCleanupStatus {
        bridge_id: record.identity.bridge_id.clone(),
        allocation_id: record.id.clone(),
        generation: record.identity.generation,
        allocation_revision: record.revision,
        target: record.placement.target().clone(),
        receipt: None,
        progress: ChildCleanupProgress::NotRequested,
        holds: Vec::new(),
        observed_at_ms: None,
        agent_capacity_released: false,
        workspace_retained: true,
        history_retained: true,
    }
}

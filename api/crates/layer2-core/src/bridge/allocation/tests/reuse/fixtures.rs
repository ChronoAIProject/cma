use super::*;

pub(super) async fn reused(
    shared_workspace: bool,
) -> (
    AllocationRecord,
    AllocationAuthority,
    ResourceAccessSnapshot,
    Agent,
) {
    let (original, original_authority, mut sandbox, mut agent) =
        super::super::channel_creation::admitted_agent();
    let mut profiles = original.placement.profiles().unwrap().clone();
    profiles.sandbox.content.max_workspaces = 4;
    profiles.sandbox.content.max_agents_per_workspace = 4;
    profiles.sandbox.digest = content_digest(&profiles.sandbox.content).unwrap();
    profiles.trust_digest =
        AllocationProfiles::digest(&profiles.sandbox, &profiles.workspace, &profiles.agent)
            .unwrap();
    sandbox.id = crate::SandboxId("sandbox-reused".into());
    sandbox.profile = profiles.sandbox.clone();
    sandbox.limits = SandboxLimits {
        max_workspaces: 4,
        max_agents_per_workspace: 4,
    };
    let mut bridge = original_authority.bridge().clone();
    let sandbox_id = sandbox.id.clone();
    let workspace_id = crate::WorkspaceId("workspace-reused".into());
    let shared_trust = BridgeSharedTrust::AllParticipantsShareFilesystemProcessesAndCredentials;
    let capacity = BridgeSharedCapacity {
        keep_other_workspaces: 1,
        keep_other_agents_per_workspace: 1,
    };
    let agent_profile = AgentProfileRef::new(profiles.agent.id.clone(), profiles.agent.revision);
    bridge.generations[0].config.placement = if shared_workspace {
        BridgePlacementPolicy::WorktreeAgentPerPartition {
            sandbox_id,
            workspace_id: workspace_id.clone(),
            agent_profile,
            shared_trust,
            capacity,
        }
    } else {
        BridgePlacementPolicy::WorkspacePerPartition {
            sandbox_id,
            workspace_profile: WorkspaceProfileRef::new(
                profiles.workspace.id.clone(),
                profiles.workspace.revision,
            ),
            agent_profile,
            shared_trust,
            capacity,
        }
    };
    let workspace = shared_workspace.then(|| Workspace {
        id: workspace_id,
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: profiles.workspace.clone(),
        name: "existing-owner-repo".into(),
        path: "/home/cma/existing-owner-repo".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("e".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, sandbox.created_at.clone()),
        next_worktree_number: 8,
        created_at: sandbox.created_at.clone(),
        ready_at: Some(sandbox.created_at.clone()),
        failed: None,
        removed_at: None,
        revision: 7,
    });
    let resources = ResourceAccessSnapshot {
        lineage: ResourceLineage {
            sandbox,
            workspace,
            worktree: None,
            agent: None,
        },
        policy: None,
        shares: Vec::new(),
    };
    let config = &bridge.generations[0].config;
    config.validate().unwrap();
    profiles.validate(config).unwrap();
    let connection = BridgeValidation::from_observation(
        &bridge.owner,
        config,
        BridgeConnectionObservation {
            connector: config.connector.clone(),
            bot_owner: bridge.owner.clone(),
            key_owner: bridge.owner.clone(),
            routes: vec![BridgeRouteObservation {
                route_id: "route-one".into(),
                owner: bridge.owner.clone(),
                bot_id: "bot-one".into(),
                key_id: "key-one".into(),
                enabled: true,
            }],
            custody_owner: bridge.owner.clone(),
            custody_revision: 1,
            key_state_version: 3,
            key_active: true,
            custody_available: true,
            callback_conflict: false,
            renewable_sandbox_authority: true,
            exact_profiles_available: true,
            existing_target: Some(resources.clone()),
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        },
    )
    .unwrap();
    let management =
        super::super::renewal::manager(bridge.clone(), "owner", OrganizationRole::Admin)
            .await
            .unwrap();
    let context = super::super::renewal::context("owner");
    let decision = AccessDecision::evaluate(
        &context,
        &resources
            .scope(&config.placement.reused_parent().unwrap())
            .unwrap(),
        &[],
        AccessPurpose::Read,
        std::time::SystemTime::now(),
    )
    .unwrap();
    let observation = AllocationReuseObservation {
        ordinary_parent_lifecycle_available: true,
        profiles_digest: profiles.trust_digest.clone(),
        runtime_model_services_skills_available: true,
        workspace_writer_coordination_available: true,
        deadline: tokio::time::Instant::now() + Duration::from_secs(15),
    };
    let (mut admission, _) = super::super::fixture();
    admission.bridge = bridge;
    let proof = AllocationReuseProof::prepare(
        &management,
        admission.identity(),
        original.pin.clone(),
        profiles,
        AuthorizedResource {
            snapshot: resources.clone(),
            decision,
        },
        connection,
        observation,
    )
    .unwrap();
    let authority = AllocationAuthority::Reuse(Box::new(proof));
    let record = AllocationReservation::new(admission, authority.clone(), 1000)
        .unwrap()
        .record()
        .clone();
    agent.sandbox_id = record.placement.target().sandbox_id.clone();
    agent.workspace_id = record.placement.target().workspace_id.clone();
    agent.creation_fingerprint = channel_fingerprint(&record).unwrap();
    (record, authority, resources, agent)
}

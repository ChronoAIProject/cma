use super::*;

pub(super) async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_804_{prefix}_{}_{sequence}", std::process::id());
    (client, name)
}

pub(super) async fn claim_interaction_fence(store: &MongoSandboxStore, id: &AgentId) -> Agent {
    use layer2_core::rfc3339_at;
    use std::time::Duration;

    // Only the lease needs database time. The cleanup fixtures use their fixed
    // timeline to publish events and drive the subsequent steps.
    let hello = store
        .client
        .database("admin")
        .run_command(doc! { "hello": 1 })
        .await
        .unwrap();
    let now = hello.get_datetime("localTime").unwrap().to_system_time();
    let claimed = store
        .claim_agent_work(
            id,
            "worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, AgentWorkStep::FenceInteraction);
    claimed
}

pub(super) fn command_uses_snapshot_read(command: &Document) -> bool {
    command
        .get_document("readConcern")
        .ok()
        .and_then(|concern| concern.get_str("level").ok())
        == Some("snapshot")
}

pub(in crate::store) fn sandbox(id: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 1,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "provider-key".into(),
                provider_state_version: 1,
                issued_at: NOW.into(),
                expires_at: "2026-10-05T00:00:00Z".into(),
                grant_fingerprint: "a".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(super) fn provisioning_sandbox(id: &str) -> Sandbox {
    let mut sandbox = sandbox(id);
    sandbox.status = SandboxStatus::Provisioning;
    sandbox.credential.key_generation = 0;
    sandbox.credential.delivered = false;
    sandbox.credential.current = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into());
    sandbox
}

pub(super) fn admission(id: &str, limit: u16) -> SandboxAdmission {
    SandboxAdmission::new(
        provisioning_sandbox(id),
        SandboxAdmissionName::Allocate("research".into()),
        limit,
        NOW.into(),
    )
    .unwrap()
}

pub(super) fn retired_sandbox(id: &str) -> Sandbox {
    let mut sandbox = sandbox(id);
    sandbox.status = SandboxStatus::Retired;
    sandbox.credential.delivered = false;
    sandbox.credential.current = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::Purge, NOW.into());
    sandbox.retired_at = Some(NOW.into());
    sandbox.purge_after = Some(NOW.into());
    sandbox
}

pub(super) fn credential(sandbox_id: &str, plaintext: &str) -> CredentialMaterial {
    CredentialMaterial {
        source: Default::default(),
        github_route: None,
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId("owner".into()),
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: "provider-key".into(),
            provider_state_version: 1,
            issued_at: NOW.into(),
            expires_at: "2026-10-05T00:00:00Z".into(),
            grant_fingerprint: "c".repeat(64),
        },
        value: SecretValue::new(plaintext.into()),
    }
}

pub(in crate::store) fn encryption_key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .expect("test encryption key")
}

pub(super) fn agent_profile(id: &str) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: AgentProfileId(id.into()),
        owner: SubjectId("owner".into()),
        name: "Research agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

pub(super) fn workspace_service_profile(id: &str, name: &str) -> WorkspaceProfile {
    WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId("owner".into()),
        name: format!("{name} profile"),
        description: "Workspace admission integration profile".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory { name: name.into() },
            name: name.into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

pub(super) fn worktree_and_agent(sandbox_id: &SandboxId) -> (Worktree, Agent) {
    let agent_id = AgentId("agt_atomic".into());
    let worktree = Worktree {
        id: WorktreeId("wkt_atomic".into()),
        workspace_id: WorkspaceId("wks_atomic".into()),
        sandbox_id: sandbox_id.clone(),
        owner: SubjectId("owner".into()),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "0123456789abcdef".into(),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent_id.clone()),
        revision: 1,
    };
    let agent = Agent {
        id: agent_id,
        worktree_id: worktree.id.clone(),
        workspace_id: worktree.workspace_id.clone(),
        sandbox_id: sandbox_id.clone(),
        owner: worktree.owner.clone(),
        creation_key: "create-atomic".into(),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_test".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: agent_profile("agp_test").draft,
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    (worktree, agent)
}

pub(super) fn workspace(sandbox: &Sandbox, id: &str, name: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_mongo".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: name.into() },
                name: name.into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: name.into(),
        path: format!("/home/cma/{name}"),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

pub(super) fn workspace_admission(workspace: Workspace) -> WorkspaceAdmission {
    WorkspaceAdmission {
        binding: None,
        workspace,
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    }
}

pub(in crate::store) fn ready_workspace(parent: &Sandbox, id: &str, name: &str) -> Workspace {
    let mut workspace = workspace(parent, id, name);
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.status = WorkspaceStatus::Ready;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    workspace.ready_at = Some(NOW.into());
    workspace
}

pub(super) fn agent_admission(workspace: &Workspace, key: &str, id: &str) -> AgentAdmission {
    AgentAdmission {
        binding: None,
        created_by: None,
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId(format!("agt_{id}")),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_agent_admission".into()),
            revision: 3,
            digest: "c".repeat(64),
            content: agent_profile("agp_agent_admission").draft,
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: key.into(),
        event_at: NOW.into(),
    }
}

pub(super) fn created_agent(
    result: AgentAdmissionResult,
) -> layer2_core::sandbox::store::AgentCreation {
    match result {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("expected a fresh agent admission"),
    }
}

pub(super) fn agent_for_worker(id: &str, ready_at: &str) -> Agent {
    let (_, mut agent) = worktree_and_agent(&SandboxId("sbx_agent_worker".into()));
    agent.id = AgentId(id.into());
    agent.worktree_id = WorktreeId(format!("wkt_{id}"));
    agent.creation_key = format!("create-{id}");
    agent.work.ready_at = ready_at.into();
    agent
}

pub(super) async fn claimed_agent_work_fixture(
    store: &MongoSandboxStore,
    suffix: &str,
) -> (Sandbox, Worktree, Agent) {
    let mut parent = sandbox(&format!("sbx_agent_work_{suffix}"));
    parent.name = suffix.into();
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = ready_workspace(
        &parent,
        &format!("wks_agent_work_{suffix}"),
        &format!("agent-work-{suffix}"),
    );
    store.create_workspace(workspace).await.unwrap();
    let (mut worktree, mut agent) = worktree_and_agent(&parent.id);
    let agent_id = AgentId(format!("agt_agent_work_{suffix}"));
    worktree.id = WorktreeId(format!("wkt_agent_work_{suffix}"));
    worktree.workspace_id = WorkspaceId(format!("wks_agent_work_{suffix}"));
    worktree.agent_id = Some(agent_id.clone());
    worktree.status = WorktreeStatus::Creating;
    agent.id = agent_id;
    agent.worktree_id = worktree.id.clone();
    agent.workspace_id = worktree.workspace_id.clone();
    agent.creation_key = format!("create-agent-work-{suffix}");
    store.create_worktree(worktree.clone()).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    let claimed = store
        .claim_agent_work(&agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    (parent, worktree, claimed)
}

pub(super) fn ready_publication_claim(mut agent: Agent, conversation_id: &str) -> Agent {
    agent.work.step = AgentWorkStep::PublishReady;
    agent.conversation.conversation_id = Some(conversation_id.into());
    agent.conversation.runtime_generation = Some(1);
    agent.work.receipts = [
        (
            AgentWorkStep::AddWorktree,
            format!("worktree:{}:add:v1", agent.worktree_id),
        ),
        (
            AgentWorkStep::MaterializeHome,
            format!("agent:{}:home:create:v1", agent.id),
        ),
        (
            AgentWorkStep::OpenConversation,
            format!("agent:{}:open:v1", agent.id),
        ),
    ]
    .into_iter()
    .map(|(step, operation_id)| AgentWorkReceipt {
        step,
        operation_id,
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    })
    .collect();
    agent
}

pub(super) async fn replace_agent_fixture(store: &MongoSandboxStore, agent: &Agent) {
    let result = store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": agent_replacement_fields(agent).unwrap() },
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

pub(super) async fn replace_worktree_fixture(store: &MongoSandboxStore, worktree: &Worktree) {
    let result = store
        .worktrees
        .update_one(
            doc! { "_id": worktree.id.as_str() },
            doc! { "$set": worktree_replacement_fields(worktree).unwrap() },
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

pub(in crate::store) async fn ready_conversation_fixture(
    store: &MongoSandboxStore,
    suffix: &str,
) -> Agent {
    let (_, mut worktree, claimed) = claimed_agent_work_fixture(store, suffix).await;
    let conversation_id = format!("conversation-{suffix}");
    let claimed = ready_publication_claim(claimed, &conversation_id);
    replace_agent_fixture(store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut ready = claimed.clone();
    ready
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    ready.conversation.last_seq = u64::try_from(log_kinds.len()).unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: ready.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: ready.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: log_kinds,
        })
        .await
        .unwrap();
    ready
}

pub(super) fn advance_claimed_agent(claimed: &Agent) -> Agent {
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("worker", fence, AgentWorkStep::MaterializeHome, NOW.into())
        .unwrap();
    next.revision += 1;
    next
}

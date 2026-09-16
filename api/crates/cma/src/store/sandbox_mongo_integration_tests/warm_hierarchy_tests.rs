use super::*;
use layer2_core::profiles::{content_digest, AgentProfileRef};
use layer2_core::sandbox::store::AgentProvenanceStore;
use layer2_core::sandbox::{
    RuntimeBinding, WarmSandbox, WarmSandboxPlan, WarmSandboxState, WarmWorkspacePlan,
};

async fn fixture() -> (
    Client,
    String,
    MongoSandboxStore,
    Sandbox,
    Workspace,
    WarmSandbox,
) {
    let (client, name) = database("warm_hierarchy").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_warm_hierarchy");
    parent.status = SandboxStatus::Installing;
    let mut candidate = workspace(&parent, "wks_candidate", "prepared");
    let mut agent = agent_admission(&candidate, "test", "candidate").profile;
    agent.digest = content_digest(&agent.content).unwrap();
    candidate.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    candidate.profile.content.agent_profiles =
        vec![AgentProfileRef::new(agent.id.clone(), agent.revision)];
    candidate.profile.digest = content_digest(&candidate.profile.content).unwrap();
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        candidate.profile.id.clone(),
        candidate.profile.revision,
    )];
    parent.profile.digest = content_digest(&parent.profile.content).unwrap();
    let mut warm = WarmSandbox::new(
        WarmSandboxPlan {
            profile: parent.profile.clone(),
            workspaces: vec![WarmWorkspacePlan {
                profile: candidate.profile.clone(),
                agents: vec![agent],
            }],
            reaches: vec![],
            runtime_version: "warm-image".into(),
            execution: Default::default(),
        },
        NOW.into(),
    )
    .unwrap();
    warm.state = WarmSandboxState::Claimed;
    warm.installed_request_sha256 = Some("a".repeat(64));
    warm.runtime = Some(RuntimeBinding {
        runtime_id: "prepared-runtime".into(),
        runtime_version: "warm-image".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: NOW.into(),
    });
    warm.workspaces[0].resolved_commit = Some("a".repeat(40));
    warm.workspaces[0].setup_ready = true;
    warm.workspaces[0].agents[0].skills = Some(vec![]);
    warm.workspaces[0].agents[0].conversation_id = Some("prepared-conversation".into());
    warm.validate().unwrap();
    parent.id = warm.id.clone();
    parent.runtime = warm.runtime.clone();
    parent.lifecycle.runtime_generation_high_water = 1;
    candidate.sandbox_id = parent.id.clone();
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .warm_sandboxes
        .insert_one(crate::store::warm_sandboxes::document(&warm).unwrap())
        .await
        .unwrap();
    (client, name, store, parent, candidate, warm)
}

async fn admit_workspace(store: &MongoSandboxStore, candidate: Workspace) -> Workspace {
    let WorkspaceAdmissionResult::Created(mut workspace) = store
        .admit_workspace(WorkspaceAdmission {
            workspace: candidate,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap()
    else {
        panic!("expected prepared workspace adoption")
    };
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.status = WorkspaceStatus::Ready;
    workspace.work.step = WorkspaceWorkStep::None;
    workspace.ready_at = Some(NOW.into());
    workspace.revision += 1;
    store.save_workspace(1, workspace.clone()).await.unwrap();
    workspace
}

async fn activate(store: &MongoSandboxStore, id: &SandboxId) {
    let mut parent = store.get_sandbox(id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.revision += 1;
    store.save_sandbox(revision, parent).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn warm_automatic_agent_is_claimed_once_by_a_new_conversation_trigger() {
    let (client, name, store, _, candidate, warm) = fixture().await;
    let workspace = admit_workspace(&store, candidate).await;
    activate(&store, &workspace.sandbox_id).await;
    let mut automatic = agent_admission(&workspace, "unused", "automatic");
    automatic.profile = warm.plan.workspaces[0].agents[0].clone();
    automatic.origin = layer2_core::sandbox::AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    automatic.first_message = None;
    automatic.title = None;
    let prepared = store
        .admit_agent(automatic)
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    assert_eq!(prepared.id, warm.workspaces[0].agents[0].id);
    let provenance = store
        .agent_provenance_snapshot(&prepared.id)
        .await
        .unwrap()
        .creation_provenance(&prepared.id)
        .unwrap();
    assert_eq!(provenance.agent_id, prepared.id);
    assert_eq!(provenance.provisioning_origin, prepared.origin);
    let mut trigger = agent_admission(&workspace, "new-conversation", "trigger");
    trigger.profile = prepared.profile.clone();
    trigger.origin = layer2_core::sandbox::AgentOrigin::Trigger {
        trigger_id: "trg_warm".into(),
        launch_id: "lch_warm".into(),
    };
    // An empty first prompt still needs an exclusive durable claim.
    trigger.first_message = None;
    let (left, right) = tokio::join!(
        store.admit_agent(trigger.clone()),
        store.admit_agent(trigger.clone())
    );
    for admitted in [left.unwrap(), right.unwrap()] {
        assert_eq!(admitted.creation().agent.id, prepared.id);
        assert_eq!(admitted.creation().agent.worktree_id, prepared.worktree_id);
    }
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .admit_agent(trigger.clone())
            .await
            .unwrap()
            .creation()
            .agent
            .id,
        prepared.id
    );
    assert_eq!(
        reopened
            .agent_provenance_snapshot(&prepared.id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        provenance
    );
    trigger.creation_key = "next-conversation".into();
    trigger.agent_id = AgentId::generate();
    trigger.worktree_id = WorktreeId::generate();
    let next = reopened.admit_agent(trigger).await.unwrap();
    assert_ne!(next.creation().agent.id, prepared.id);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn warm_hierarchy_adopts_once_replays_and_reserves_worktree_numbers() {
    let (client, name, store, _, candidate, warm) = fixture().await;
    let workspace = admit_workspace(&store, candidate.clone()).await;
    assert_eq!(workspace.id, warm.workspaces[0].id);
    assert_eq!(workspace.next_worktree_number, 2);
    assert_eq!(
        store
            .admit_workspace(WorkspaceAdmission {
                workspace: candidate,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .unwrap(),
        WorkspaceAdmissionResult::Existing(workspace.clone())
    );
    activate(&store, &workspace.sandbox_id).await;
    let mut request = agent_admission(&workspace, "trigger-claim", "first");
    request.profile = warm.plan.workspaces[0].agents[0].clone();
    request.origin = layer2_core::sandbox::AgentOrigin::Trigger {
        trigger_id: "trg_warm".into(),
        launch_id: "lch_warm_first".into(),
    };
    let (left, right) = tokio::join!(
        store.admit_agent(request.clone()),
        store.admit_agent(request.clone())
    );
    let mut created = None;
    let mut replayed = None;
    for result in [left.unwrap(), right.unwrap()] {
        match result {
            AgentAdmissionResult::Created(row) => created = Some(row),
            AgentAdmissionResult::Existing(row) => replayed = Some(row),
        }
    }
    let created = created.unwrap();
    assert_eq!(replayed.unwrap(), created);
    let prepared = &warm.workspaces[0].agents[0];
    assert_eq!(created.agent.id, prepared.id);
    let provenance = store
        .agent_provenance_snapshot(&prepared.id)
        .await
        .unwrap()
        .creation_provenance(&prepared.id)
        .unwrap();
    assert_eq!(provenance.agent_id, prepared.id);
    assert_eq!(provenance.worktree_id, prepared.worktree_id);
    assert!(matches!(
        provenance.creation_origin,
        layer2_core::sandbox::AgentCreationOrigin::Trigger { .. }
    ));
    assert_eq!(created.worktree.id, prepared.worktree_id);
    assert_eq!(
        created.agent.work.warm_conversation_id,
        prepared.conversation_id
    );
    assert_eq!(created.worktree.name, "wt-1");
    assert_eq!(created.agent.work.next_fence, 2);
    // Faster Trigger scheduling can win the race with automatic reconciliation.
    // The automatic child must recognize the prepared slot already adopted by it.
    let mut automatic = agent_admission(&workspace, "automatic", "automatic");
    automatic.profile = created.agent.profile.clone();
    automatic.origin = layer2_core::sandbox::AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    automatic.first_message = None;
    automatic.title = None;
    let reconciled = store.admit_agent(automatic).await.unwrap();
    assert!(matches!(&reconciled, AgentAdmissionResult::Existing(_)));
    assert_eq!(reconciled.creation().agent.id, created.agent.id);
    assert_eq!(
        store
            .agent_provenance_snapshot(&prepared.id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        provenance
    );
    request.creation_key = "another-user-action".into();
    request.agent_id = AgentId::generate();
    request.worktree_id = WorktreeId::generate();
    let another = created_agent(store.admit_agent(request).await.unwrap());
    assert_ne!(another.agent.id, prepared.id);
    assert_eq!(another.worktree.name, "wt-3");
    assert!(another.agent.work.warm_conversation_id.is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn warm_hierarchy_refuses_another_runtime_or_unpinned_agent() {
    let (client, name, store, mut parent, candidate, warm) = fixture().await;
    let workspace = admit_workspace(&store, candidate).await;
    activate(&store, &workspace.sandbox_id).await;
    let mut changed = agent_admission(&workspace, "changed-profile", "changed");
    changed.profile = warm.plan.workspaces[0].agents[0].clone();
    changed.profile.content.system_prompt = "different profile".into();
    changed.profile.digest = content_digest(&changed.profile.content).unwrap();
    let other = created_agent(store.admit_agent(changed).await.unwrap());
    assert_ne!(other.agent.id, warm.workspaces[0].agents[0].id);
    assert!(other.agent.work.warm_conversation_id.is_none());
    parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
    parent.revision += 1;
    store.save_sandbox(revision, parent).await.unwrap();
    let mut request = agent_admission(&workspace, "new-runtime", "new");
    request.profile = warm.plan.workspaces[0].agents[0].clone();
    let other = created_agent(store.admit_agent(request).await.unwrap());
    assert_ne!(other.agent.id, warm.workspaces[0].agents[0].id);
    assert!(other.agent.work.warm_conversation_id.is_none());
    client.database(&name).drop().await.unwrap();
}

use super::*;
use crate::profiles::AgentProfileSnapshot;
use crate::sandbox::store::{AgentAdmission, AgentAdmissionStore};
use crate::WorktreeId;

#[tokio::test]
async fn trigger_reuses_automatic_agent_and_initializes_its_first_prompt_once() {
    let (service, store, profiles, workspace) =
        setup_profile_children(SandboxStatus::Active, WorkspaceStatus::Ready, 1).await;
    service
        .reconcile_profile_agents(&workspace.sandbox_id)
        .await
        .unwrap();
    let before = store
        .list_agents(&workspace.owner, &Default::default())
        .await
        .unwrap()
        .items
        .remove(0);
    let owner = launch_caller("owner", Capability::CreateSandbox);
    let request = request(
        workspace.profile.content.agent_profiles[0].clone(),
        AgentOrigin::Trigger {
            trigger_id: "trg_reuse".into(),
            launch_id: "lch_first".into(),
        },
    );
    let first = service
        .create_or_reuse_for_launch(
            &owner,
            Capability::CreateSandbox,
            &workspace.id,
            request.clone(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(first.id, before.id);
    assert_eq!(first.worktree_id, before.worktree_id);
    assert_eq!(first.origin, before.origin);
    assert_eq!(
        first.conversation.first_message.as_ref().unwrap().text,
        "Inspect the proof."
    );
    let again = service
        .create_or_reuse_for_launch(
            &owner,
            Capability::CreateSandbox,
            &workspace.id,
            request,
            true,
        )
        .await
        .unwrap();
    assert_eq!(again.conversation, first.conversation);
    assert_eq!(
        store
            .list_agents(&workspace.owner, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(uses(&profiles, &first.profile.id).await, 1);
    assert_eq!(
        service
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn trigger_and_automatic_profile_admission_race_for_one_agent_and_worktree() {
    let (service, store, _, workspace) =
        setup_profile_children(SandboxStatus::Active, WorkspaceStatus::Ready, 1).await;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..8 {
        let service = service.clone();
        let workspace = workspace.clone();
        tasks.spawn(async move {
            if index % 2 == 0 {
                service
                    .reconcile_profile_agents(&workspace.sandbox_id)
                    .await
                    .map(|_| ())
            } else {
                let request = request(
                    workspace.profile.content.agent_profiles[0].clone(),
                    AgentOrigin::Trigger {
                        trigger_id: "trg_race".into(),
                        launch_id: format!("lch_{index}"),
                    },
                );
                service
                    .create_or_reuse_for_launch(
                        &launch_caller("owner", Capability::CreateSandbox),
                        Capability::CreateSandbox,
                        &workspace.id,
                        request,
                        true,
                    )
                    .await
                    .map(|_| ())
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
    let agents = store
        .list_agents(&workspace.owner, &Default::default())
        .await
        .unwrap()
        .items;
    assert_eq!(agents.len(), 1);
    assert!(agents[0].conversation.first_message.is_some());
    assert_eq!(
        store
            .list_worktrees(&workspace.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn automatic_profile_agents_recover_ready_workspaces_once_across_races_and_restart() {
    let (service, store, profiles, workspace) =
        setup_profile_children(SandboxStatus::Active, WorkspaceStatus::Ready, 2).await;
    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let service = service.clone();
        let sandbox_id = workspace.sandbox_id.clone();
        attempts.spawn(async move { service.reconcile_profile_agents(&sandbox_id).await });
    }
    let mut created = 0;
    while let Some(result) = attempts.join_next().await {
        created += result.unwrap().unwrap();
    }
    assert_eq!(created, 2);
    let agents = store
        .list_agents(&workspace.owner, &Default::default())
        .await
        .unwrap()
        .items;
    assert_eq!(agents.len(), 2);
    for agent in &agents {
        let AgentOrigin::WorkspaceProfile { index } = agent.origin else {
            panic!("automatic origin is required");
        };
        assert_eq!(
            agent.profile.id,
            workspace.profile.content.agent_profiles[usize::from(index)].id
        );
        assert_eq!(agent.status, AgentStatus::Starting);
        assert!(agent.conversation.first_message.is_none());
        assert_eq!(uses(&profiles, &agent.profile.id).await, 1);
    }
    assert_eq!(
        store
            .list_worktrees(&workspace.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    let restarted = AgentService::new(
        store.clone(),
        profiles.clone(),
        TestAgentRuntimeRegistry::new(false),
        agent_capabilities(),
    );
    assert_eq!(
        restarted
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .list_agents(&workspace.owner, &Default::default())
            .await
            .unwrap()
            .items,
        agents
    );
    for reference in &workspace.profile.content.agent_profiles {
        profiles
            .retire_agent_profile(&reference.id, &workspace.owner, NOW.into())
            .await
            .unwrap();
    }
    assert_eq!(
        restarted
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn automatic_profile_agents_do_not_replace_stopped_or_deleted_children() {
    let (service, store, _, workspace) =
        setup_profile_children(SandboxStatus::Active, WorkspaceStatus::Ready, 1).await;
    assert_eq!(
        service
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        1
    );
    let agent = store
        .list_agents(&workspace.owner, &Default::default())
        .await
        .unwrap()
        .items
        .remove(0);
    let scheduled = service
        .stop(&caller("owner", true), &agent.id, AgentStopReason::Owner)
        .await
        .unwrap();
    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, NOW.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            NOW.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store.save_agent(scheduled.revision, stopped).await.unwrap();
    assert_eq!(
        service
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
    let deleting = service
        .delete(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    let mut deleted = deleting.clone();
    deleted.work.schedule(AgentWorkStep::None, NOW.into());
    deleted
        .transition(AgentStatus::Deleted, NOW.into(), None)
        .unwrap();
    store.save_agent(deleting.revision, deleted).await.unwrap();
    assert_eq!(
        service
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .list_agents(&workspace.owner, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn automatic_profile_agents_do_not_revive_or_create_before_readiness() {
    for (sandbox_status, workspace_status) in [
        (SandboxStatus::Installing, WorkspaceStatus::Ready),
        (SandboxStatus::Paused, WorkspaceStatus::Ready),
        (SandboxStatus::Active, WorkspaceStatus::Preparing),
        (SandboxStatus::Active, WorkspaceStatus::Removed),
    ] {
        let (service, store, _, workspace) =
            setup_profile_children(sandbox_status, workspace_status, 1).await;
        assert_eq!(
            service
                .reconcile_profile_agents(&workspace.sandbox_id)
                .await
                .unwrap(),
            0
        );
        assert!(store
            .list_agents(&workspace.owner, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn automatic_profile_admission_refuses_unpinned_children_and_public_origin_injection() {
    for corruption in 0..3 {
        let (service, store, profiles, workspace) =
            setup_profile_children(SandboxStatus::Active, WorkspaceStatus::Ready, 1).await;
        let reference = &workspace.profile.content.agent_profiles[0];
        assert!(matches!(
            service
                .create(
                    &caller("owner", true),
                    &workspace.id,
                    request(
                        reference.clone(),
                        AgentOrigin::WorkspaceProfile { index: 0 }
                    )
                )
                .await,
            Err(AgentServiceError::InvalidRequest(_))
        ));
        let revision = profiles
            .agent_profile_revision(&reference.id, reference.revision)
            .await
            .unwrap()
            .unwrap();
        let mut admission = AgentAdmission {
            created_by: None,
            owner: workspace.owner.clone(),
            workspace_id: workspace.id.clone(),
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: AgentProfileSnapshot::from(&revision),
            capabilities: agent_capabilities(),
            title: None,
            first_message: None,
            origin: AgentOrigin::WorkspaceProfile { index: 0 },
            creation_key: crate::sandbox::agent::profile_agent_creation_key(&workspace.id, 0),
            event_at: NOW.into(),
        };
        match corruption {
            0 => admission.profile.revision += 1,
            1 => admission.creation_key.push_str("-changed"),
            _ => admission.origin = AgentOrigin::WorkspaceProfile { index: 1 },
        }
        assert!(matches!(
            store.admit_agent(admission).await,
            Err(SandboxStoreError::InvalidTransaction(_))
        ));
        assert!(store
            .list_agents(&workspace.owner, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

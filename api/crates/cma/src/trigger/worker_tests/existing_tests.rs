use layer2_core::sandbox::{
    AgentOrigin, AgentStatus, FirstMessageDelivery, SandboxOrigin, SandboxStatus, WorkspaceOrigin,
    WorkspaceStatus,
};
use layer2_core::trigger_domain::{
    LaunchAuthorityClass, LaunchEffectKind, LaunchState, LaunchStepKey, LaunchStepStatus,
};
use layer2_core::{AgentId, SandboxId, WorkspaceId};

use super::super::worker::LaunchResourceDriver;
use super::fixtures_tests::{
    agent_resource, frozen_inputs, reused_resolution, sandbox_resource, stamp, workspace_resource,
    Harness,
};

fn existing_harness() -> Harness {
    let sandbox_id = SandboxId("sbx_existing".into());
    let workspace_id = WorkspaceId("wks_existing".into());
    let agent_id = AgentId("agt_existing".into());
    let harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(
            sandbox_id.clone(),
            Some(workspace_id.clone()),
            Some(agent_id.clone()),
        ),
        Vec::new(),
        false,
    );
    let mut launch = harness.launch();
    launch.first_message = FirstMessageDelivery::Completed;
    let step = launch
        .steps
        .iter_mut()
        .find(|step| step.key == LaunchStepKey::FirstMessage)
        .unwrap();
    step.status = LaunchStepStatus::Completed;
    step.started_at = Some(stamp(0));
    step.completed_at = Some(stamp(0));
    harness.store.insert(launch.clone());
    let previous = frozen_inputs(
        harness.inputs.trigger_id.clone(),
        "previous",
        3,
        "Original request",
        true,
    );
    harness.driver.install_context(previous.clone());
    harness.driver.put_sandbox(sandbox_resource(
        &previous.profiles,
        launch.owner.clone(),
        sandbox_id.clone(),
        SandboxStatus::Paused,
        SandboxOrigin::Manual,
    ));
    harness.driver.put_workspace(workspace_resource(
        &previous.profiles,
        launch.owner.clone(),
        sandbox_id.clone(),
        workspace_id.clone(),
        WorkspaceStatus::Ready,
        WorkspaceOrigin::SandboxProfile { index: 0 },
    ));
    harness.driver.put_agent(agent_resource(
        &previous.profiles,
        launch.owner,
        sandbox_id,
        workspace_id,
        agent_id.clone(),
        AgentStatus::Paused,
        AgentOrigin::Trigger {
            trigger_id: launch.trigger_id.to_string(),
            launch_id: "lch_previous".into(),
        },
        Some("Original request".into()),
    ));
    harness.driver.complete_first_message(&agent_id, 0);
    harness
}

#[tokio::test]
async fn existing_conversation_revives_original_parents_without_resending_after_republish() {
    let harness = existing_harness();
    let original = harness.launch();
    let sandbox_id = original.resolution.sandbox.id().unwrap();
    let agent_id = original.resolution.agent.id().unwrap();
    let before = harness.driver.agent_value(agent_id);
    harness.run(1).await;
    assert_eq!(harness.launch().state, LaunchState::SandboxPending);
    assert!(harness.launch().ready_at.is_none());
    assert_eq!(
        harness.driver.sandbox_value(sandbox_id).status,
        SandboxStatus::Reviving
    );
    harness.run(2).await;
    assert!(harness.launch().blocked_reason.is_some());
    harness
        .driver
        .mutate_sandbox(sandbox_id, |sandbox| sandbox.status = SandboxStatus::Active);
    harness
        .driver
        .mutate_agent(agent_id, |agent| agent.status = AgentStatus::Ready);
    for minute in 3..=7 {
        harness.run(minute).await;
    }
    let ready = harness.launch();
    assert_eq!(ready.state, LaunchState::Ready);
    assert_eq!(ready.resolution, original.resolution);
    assert_eq!(harness.driver.resource_counts(), (1, 1, 1));
    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, LaunchEffectKind::ReviveSandbox);
    let after = harness.driver.agent_value(agent_id);
    assert_eq!(after.conversation, before.conversation);
    assert_eq!(after.worktree_id, before.worktree_id);
    assert_eq!(after.profile, before.profile);
    assert!(harness
        .driver
        .profile_queries()
        .iter()
        .all(|target| target.agent_profile.id == before.profile.id));
}

#[tokio::test]
async fn existing_conversation_foreign_parent_is_refused_before_revival() {
    let harness = existing_harness();
    let workspace_id = harness.launch().resolution.workspace.id().unwrap().clone();
    let mut workspace = harness
        .driver
        .workspace(&workspace_id)
        .await
        .unwrap()
        .unwrap();
    workspace.owner = layer2_core::SubjectId::new("another-owner");
    harness.driver.put_workspace(workspace);
    harness.run(1).await;
    assert_eq!(harness.launch().state, LaunchState::FailedTerminal);
    assert!(harness.driver.calls().is_empty());
}

use std::sync::Arc;

use layer2_core::sandbox::{
    AgentOrigin, AgentStatus, FirstMessageRecord, SandboxOrigin, SandboxStatus, WorkspaceOrigin,
    WorkspaceStatus,
};
use layer2_core::trigger_domain::{
    LaunchAuthorityClass, LaunchEffectKind, LaunchResolution, LaunchState, ResolvedResource,
    TriggerId,
};
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId};

use super::fixtures_tests::{
    agent_resource, frozen_inputs, reused_resolution, sandbox_resource, started_launch,
    test_worker, time, workspace_resource, FakeDriver, Harness, TestLaunchStore,
};

#[tokio::test]
async fn due_sweep_continues_after_one_launch_returns_a_store_error() {
    let inputs = frozen_inputs(
        TriggerId("trg_sweep".into()),
        "sweep",
        1,
        "Sweep message",
        false,
    );
    let store = Arc::new(TestLaunchStore::default());
    let driver = FakeDriver::new(inputs.clone());
    let bad = started_launch(
        &inputs,
        LaunchAuthorityClass::Visitor,
        LaunchResolution::pending(),
        Vec::new(),
    );
    let good = started_launch(
        &inputs,
        LaunchAuthorityClass::Visitor,
        LaunchResolution::pending(),
        Vec::new(),
    );
    let bad_id = bad.id.clone();
    let good_id = good.id.clone();
    store.insert(bad);
    store.insert(good);
    store.fail_next_claim(bad_id.clone());
    let worker = test_worker(Arc::clone(&store), Arc::clone(&driver), "sweep-worker");

    assert!(worker.run_due(time(1)).await.is_err());
    assert_eq!(store.stored(&bad_id).state, LaunchState::Resolving);
    assert_eq!(store.stored(&good_id).state, LaunchState::SandboxPending);
    assert_eq!(driver.calls().len(), 1);
}

#[tokio::test]
async fn invalid_effect_result_fails_closed_without_recording_the_resource() {
    let harness = Harness::pending();
    harness
        .driver
        .set_invalid_result(LaunchEffectKind::CreateSandbox, "not-a-sandbox-id");
    harness.run(1).await;

    let failed = harness.launch();
    assert_eq!(failed.state, LaunchState::FailedTerminal);
    assert_eq!(
        failed.failure.as_ref().unwrap().code,
        "launch_effect_result_invalid"
    );
    assert!(failed.resolution.sandbox.is_pending());
}

#[tokio::test]
async fn sandbox_owner_snapshot_and_origin_crossings_fail_closed() {
    let owner_id = SandboxId("sbx_wrong_owner".into());
    let wrong_owner = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(owner_id.clone(), None, None),
        Vec::new(),
        false,
    );
    wrong_owner.driver.put_sandbox(sandbox_resource(
        &wrong_owner.inputs.profiles,
        SubjectId::new("another-owner"),
        owner_id,
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    ));
    wrong_owner.run(1).await;
    assert_terminal_code(&wrong_owner, "launch_sandbox_invalid");

    let snapshot_id = SandboxId("sbx_wrong_snapshot".into());
    let wrong_snapshot = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(snapshot_id.clone(), None, None),
        Vec::new(),
        false,
    );
    let mut sandbox = sandbox_resource(
        &wrong_snapshot.inputs.profiles,
        wrong_snapshot.launch().owner,
        snapshot_id,
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    );
    sandbox.profile.content.memory_mib += 1;
    wrong_snapshot.driver.put_sandbox(sandbox);
    wrong_snapshot.run(1).await;
    assert_terminal_code(&wrong_snapshot, "launch_sandbox_invalid");

    let origin_id = SandboxId("sbx_wrong_origin".into());
    let wrong_origin = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(origin_id.clone(), None, None),
        Vec::new(),
        false,
    );
    wrong_origin.driver.put_sandbox(sandbox_resource(
        &wrong_origin.inputs.profiles,
        wrong_origin.launch().owner,
        origin_id,
        SandboxStatus::Active,
        SandboxOrigin::Trigger {
            launch_id: "x".repeat(129),
        },
    ));
    wrong_origin.run(1).await;
    assert_terminal_code(&wrong_origin, "launch_sandbox_invalid");

    let created_origin = Harness::pending();
    created_origin.run(1).await;
    let created_id = created_origin
        .launch()
        .resolution
        .sandbox
        .id()
        .unwrap()
        .clone();
    created_origin
        .driver
        .mutate_sandbox(&created_id, |sandbox| {
            sandbox.origin = SandboxOrigin::Manual
        });
    created_origin.run(2).await;
    assert_terminal_code(&created_origin, "launch_sandbox_invalid");
}

#[tokio::test]
async fn workspace_snapshot_and_agent_origin_crossings_fail_closed() {
    let sandbox_id = SandboxId("sbx_guardrail".into());
    let workspace_id = WorkspaceId("wks_wrong_snapshot".into());
    let workspace_harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(sandbox_id.clone(), Some(workspace_id.clone()), None),
        Vec::new(),
        false,
    );
    let owner = workspace_harness.launch().owner;
    workspace_harness.driver.put_sandbox(sandbox_resource(
        &workspace_harness.inputs.profiles,
        owner.clone(),
        sandbox_id.clone(),
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    ));
    let mut workspace = workspace_resource(
        &workspace_harness.inputs.profiles,
        owner,
        sandbox_id,
        workspace_id,
        WorkspaceStatus::Ready,
        WorkspaceOrigin::Manual,
    );
    workspace.profile.digest = "f".repeat(64);
    workspace_harness.driver.put_workspace(workspace);
    workspace_harness.run(1).await;
    workspace_harness.run(2).await;
    workspace_harness.run(3).await;
    assert_terminal_code(&workspace_harness, "launch_workspace_invalid");

    let sandbox_id = SandboxId("sbx_agent_guardrail".into());
    let workspace_id = WorkspaceId("wks_agent_guardrail".into());
    let agent_id = AgentId("agt_wrong_origin".into());
    let agent_harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(
            sandbox_id.clone(),
            Some(workspace_id.clone()),
            Some(agent_id.clone()),
        ),
        Vec::new(),
        false,
    );
    let owner = agent_harness.launch().owner;
    agent_harness.driver.put_sandbox(sandbox_resource(
        &agent_harness.inputs.profiles,
        owner.clone(),
        sandbox_id.clone(),
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    ));
    agent_harness.driver.put_workspace(workspace_resource(
        &agent_harness.inputs.profiles,
        owner.clone(),
        sandbox_id.clone(),
        workspace_id.clone(),
        WorkspaceStatus::Ready,
        WorkspaceOrigin::Manual,
    ));
    agent_harness.driver.put_agent(agent_resource(
        &agent_harness.inputs.profiles,
        owner,
        sandbox_id,
        workspace_id,
        agent_id,
        AgentStatus::Ready,
        AgentOrigin::Manual,
        Some("Inspect the workspace".into()),
    ));
    for minute in 1..=5 {
        agent_harness.run(minute).await;
    }
    assert_terminal_code(&agent_harness, "launch_agent_invalid");
}

#[tokio::test]
async fn mismatched_first_message_evidence_fails_closed() {
    let harness = Harness::pending();
    for minute in 1..=5 {
        harness.run(minute).await;
    }
    let agent_id = harness.launch().resolution.agent.id().unwrap().clone();
    harness.driver.mutate_agent(&agent_id, |agent| {
        agent.conversation.first_message = Some(
            FirstMessageRecord::new(
                agent.creation_key.clone(),
                "A message from another launch",
                agent.created_at.clone(),
            )
            .unwrap(),
        );
    });
    harness.run(6).await;
    assert_terminal_code(&harness, "launch_first_message_invalid");
}

#[tokio::test]
async fn queued_first_message_is_supplied_once_and_never_dispatched_by_launch_restarts() {
    let harness = Harness::pending();
    for minute in 1..=6 {
        harness.run(minute).await;
    }
    assert_eq!(harness.launch().state, LaunchState::MessagePending);
    let agent_id = harness.launch().resolution.agent.id().unwrap().clone();
    let restart_one = test_worker(
        Arc::clone(&harness.store),
        Arc::clone(&harness.driver),
        "message-observer-one",
    );
    assert!(restart_one
        .run_one(&harness.launch_id, time(7))
        .await
        .unwrap());
    let restart_two = test_worker(
        Arc::clone(&harness.store),
        Arc::clone(&harness.driver),
        "message-observer-two",
    );
    assert!(restart_two
        .run_one(&harness.launch_id, time(8))
        .await
        .unwrap());

    let create_agent_calls = harness
        .driver
        .calls()
        .into_iter()
        .filter(|call| call.kind == LaunchEffectKind::CreateAgent)
        .collect::<Vec<_>>();
    assert_eq!(create_agent_calls.len(), 1);
    assert_eq!(
        create_agent_calls[0].first_message.as_deref(),
        Some("Inspect the workspace")
    );
    let agent = harness.driver.agent_value(&agent_id);
    assert_eq!(
        agent.conversation.first_message.unwrap().state,
        layer2_core::sandbox::FirstMessageDelivery::Queued
    );
    assert_eq!(harness.launch().state, LaunchState::MessagePending);
    assert!(matches!(
        harness.launch().resolution.agent,
        ResolvedResource::Created(_)
    ));
}

fn assert_terminal_code(harness: &Harness, code: &str) {
    let launch = harness.launch();
    assert_eq!(launch.state, LaunchState::FailedTerminal);
    assert_eq!(launch.failure.as_ref().unwrap().code, code);
}

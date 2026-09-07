use std::sync::Arc;

use layer2_core::sandbox::{
    SandboxOrigin, SandboxStatus, TypedFailure, WorkspaceOrigin, WorkspaceStatus,
};
use layer2_core::trigger_domain::{
    LaunchAuthorityClass, LaunchBlockedReason, LaunchEffectKind, LaunchEffectState,
    LaunchResolution, LaunchState, LaunchStore, ResolvedResource, ServiceRequirement,
    ServiceRequirementState,
};
use layer2_core::{Capability, SandboxId, ServiceId, WorkspaceId};

use super::fixtures_tests::{
    reused_resolution, sandbox_resource, stamp, test_worker, time, workspace_resource, Harness,
};

#[tokio::test]
async fn checkpoint_held_child_creation_waits_without_spending_dependency_retries() {
    for start in [3, 5] {
        let harness = Harness::pending();
        for minute in 1..start {
            harness.run(minute).await;
        }
        let sandbox_id = harness.launch().resolution.sandbox.id().unwrap().clone();
        harness.driver.mutate_sandbox(&sandbox_id, |sandbox| {
            sandbox.begin_checkpoint_capture(stamp(start)).unwrap();
        });
        let before = harness.launch();
        let calls = harness.driver.calls().len();
        for minute in start..start + 12 {
            harness.run(minute).await;
            let waiting = harness.launch();
            assert_eq!(waiting.state, before.state);
            assert_eq!(waiting.work.attempts, 0);
            assert!(waiting.work.effect.is_none());
            assert!(waiting.failure.is_none());
            assert!(waiting
                .steps
                .iter()
                .any(|step| step.safe_detail == "Waiting for Sandbox checkpoint"));
        }
        assert_eq!(harness.driver.calls().len(), calls);
        release_checkpoint(&harness, &sandbox_id, start + 12);
        harness.run(start + 12).await;
        assert_eq!(harness.driver.calls().len(), calls + 1);
        assert!(!harness.launch().state.is_terminal());
    }
}

#[tokio::test]
async fn checkpoint_racing_child_admission_preserves_effect_identity_across_wait_and_restart() {
    for (start, kind) in [
        (3, LaunchEffectKind::CreateWorkspace),
        (5, LaunchEffectKind::CreateAgent),
    ] {
        let harness = Harness::pending();
        for minute in 1..start {
            harness.run(minute).await;
        }
        harness.driver.checkpoint_before_next_creation(kind);
        harness.run(start).await;
        let waiting = harness.launch();
        assert_eq!(waiting.work.attempts, 0);
        let effect = waiting.work.effect.unwrap();
        assert_eq!(effect.state, LaunchEffectState::RecoveryRequired);
        let restarted = test_worker(
            Arc::clone(&harness.store),
            Arc::clone(&harness.driver),
            "checkpoint-restarted",
        );
        for minute in start + 1..start + 12 {
            assert!(restarted
                .run_one(&harness.launch_id, time(minute))
                .await
                .unwrap());
            let waiting = harness.launch();
            assert_eq!(waiting.work.attempts, 0);
            let retained = waiting.work.effect.unwrap();
            assert_eq!(retained.state, LaunchEffectState::RecoveryRequired);
            assert_eq!(retained.intent_key, effect.intent_key);
            assert_eq!(retained.attempt, effect.attempt);
        }
        let sandbox_id = harness.launch().resolution.sandbox.id().unwrap().clone();
        release_checkpoint(&harness, &sandbox_id, start + 12);
        assert!(restarted
            .run_one(&harness.launch_id, time(start + 12))
            .await
            .unwrap());
        assert!(harness.launch().work.effect.is_none());
        assert_eq!(
            harness
                .driver
                .calls()
                .iter()
                .filter(|call| call.kind == kind)
                .count(),
            1
        );
    }
}

fn release_checkpoint(harness: &Harness, sandbox_id: &SandboxId, minute: u64) {
    harness.driver.mutate_sandbox(sandbox_id, |sandbox| {
        sandbox
            .claim_checkpoint_capture("capture".into(), stamp(minute), stamp(minute + 1))
            .unwrap();
        let fence = sandbox
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .work
            .lease
            .as_ref()
            .unwrap()
            .fence;
        sandbox
            .require_checkpoint_writer_resume("capture", fence, stamp(minute))
            .unwrap();
        sandbox
            .claim_checkpoint_capture("resume".into(), stamp(minute), stamp(minute + 1))
            .unwrap();
        let fence = sandbox
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .work
            .lease
            .as_ref()
            .unwrap()
            .fence;
        sandbox
            .complete_checkpoint_writer_resume("resume", fence, stamp(minute))
            .unwrap();
    });
}

#[tokio::test]
async fn expired_executing_effect_replays_one_external_resource_after_restart() {
    let harness = Harness::pending();
    harness
        .driver
        .set_crash_once(LaunchEffectKind::CreateSandbox);
    let worker = Arc::clone(&harness.worker);
    let launch_id = harness.launch_id.clone();
    let crashed = tokio::spawn(async move { worker.run_one(&launch_id, time(1)).await }).await;
    assert!(crashed.unwrap_err().is_panic());

    let executing = harness.launch();
    assert_eq!(
        executing.work.effect.as_ref().unwrap().state,
        LaunchEffectState::Executing
    );
    assert_eq!(harness.driver.resource_counts(), (1, 0, 0));

    let recovered_claim = harness
        .store
        .claim_launch(&harness.launch_id, "recovery-inspector", stamp(2), stamp(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_claim.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired
    );

    let restarted = test_worker(
        Arc::clone(&harness.store),
        Arc::clone(&harness.driver),
        "restarted-launch-worker",
    );
    assert!(restarted
        .run_one(&harness.launch_id, time(4))
        .await
        .unwrap());
    let settled = harness.launch();
    assert_eq!(settled.state, LaunchState::SandboxPending);
    assert!(settled.work.effect.is_none());
    assert!(matches!(
        settled.resolution.sandbox,
        ResolvedResource::Created(_)
    ));
    assert_eq!(harness.driver.resource_counts(), (1, 0, 0));
    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls
        .iter()
        .all(|call| call.kind == LaunchEffectKind::CreateSandbox));
    assert_eq!(calls[0].result_id, calls[1].result_id);
}

#[tokio::test]
async fn reused_paused_sandbox_reblocks_until_revive_becomes_active() {
    let sandbox_id = SandboxId("sbx_paused".into());
    let harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(sandbox_id.clone(), None, None),
        Vec::new(),
        false,
    );
    harness.driver.put_sandbox(sandbox_resource(
        &harness.inputs.profiles,
        harness.launch().owner,
        sandbox_id.clone(),
        SandboxStatus::Paused,
        SandboxOrigin::Manual,
    ));

    harness.run(1).await;
    assert_eq!(
        harness.driver.sandbox_value(&sandbox_id).status,
        SandboxStatus::Reviving
    );
    harness.run(2).await;
    let blocked = harness.launch();
    assert_eq!(
        blocked.blocked_reason,
        Some(LaunchBlockedReason::SandboxPausedReviving)
    );
    let blocked_revision = blocked.revision;

    harness.run(3).await;
    let reblocked = harness.launch();
    assert_eq!(
        reblocked.blocked_reason,
        Some(LaunchBlockedReason::SandboxPausedReviving)
    );
    assert!(reblocked.revision > blocked_revision);

    harness.driver.mutate_sandbox(&sandbox_id, |sandbox| {
        sandbox.status = SandboxStatus::Active;
    });
    harness.run(4).await;
    let resumed = harness.launch();
    assert_eq!(resumed.blocked_reason, None);
    assert_eq!(resumed.state, LaunchState::WorkspacePending);
    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, LaunchEffectKind::ReviveSandbox);
    assert_eq!(calls[0].capabilities, vec![Capability::LaunchTriggers]);
    assert_eq!(calls[0].authority, None);
}

#[tokio::test]
async fn sandbox_paused_after_resolution_settles_revive_before_polling_again() {
    let sandbox_id = SandboxId("sbx_paused_race".into());
    let harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(sandbox_id.clone(), None, None),
        Vec::new(),
        false,
    );
    harness.driver.put_sandbox(sandbox_resource(
        &harness.inputs.profiles,
        harness.launch().owner,
        sandbox_id.clone(),
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    ));
    harness.run(1).await;
    harness.driver.mutate_sandbox(&sandbox_id, |sandbox| {
        sandbox.status = SandboxStatus::Paused;
        sandbox.runtime = None;
    });

    harness.run(2).await;
    let revived = harness.launch();
    assert_eq!(revived.state, LaunchState::SandboxPending);
    assert!(revived.work.effect.is_none());
    assert_eq!(
        harness.driver.sandbox_value(&sandbox_id).status,
        SandboxStatus::Reviving
    );

    harness.run(3).await;
    assert_eq!(
        harness.launch().blocked_reason,
        Some(LaunchBlockedReason::SandboxPausedReviving)
    );
    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, LaunchEffectKind::ReviveSandbox);
}

#[tokio::test]
async fn failed_reused_sandbox_and_workspace_are_replaced() {
    let old_sandbox = SandboxId("sbx_failed_reused".into());
    let sandbox_harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(old_sandbox.clone(), None, None),
        Vec::new(),
        false,
    );
    sandbox_harness.driver.put_sandbox(sandbox_resource(
        &sandbox_harness.inputs.profiles,
        sandbox_harness.launch().owner,
        old_sandbox.clone(),
        SandboxStatus::Failed,
        SandboxOrigin::Manual,
    ));
    sandbox_harness.run(1).await;
    sandbox_harness.run(2).await;
    let replaced_sandbox = sandbox_harness.launch();
    assert!(matches!(
        &replaced_sandbox.resolution.sandbox,
        ResolvedResource::Created(id) if id != &old_sandbox
    ));
    assert_eq!(
        sandbox_harness.driver.calls()[0].kind,
        LaunchEffectKind::CreateSandbox
    );

    let live_sandbox = SandboxId("sbx_workspace_parent".into());
    let failed_workspace = WorkspaceId("wks_failed_reused".into());
    let workspace_harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        reused_resolution(live_sandbox.clone(), Some(failed_workspace.clone()), None),
        Vec::new(),
        false,
    );
    let owner = workspace_harness.launch().owner;
    workspace_harness.driver.put_sandbox(sandbox_resource(
        &workspace_harness.inputs.profiles,
        owner.clone(),
        live_sandbox.clone(),
        SandboxStatus::Active,
        SandboxOrigin::Manual,
    ));
    workspace_harness.driver.put_workspace(workspace_resource(
        &workspace_harness.inputs.profiles,
        owner,
        live_sandbox,
        failed_workspace.clone(),
        WorkspaceStatus::Failed,
        WorkspaceOrigin::Manual,
    ));
    for minute in 1..=4 {
        workspace_harness.run(minute).await;
    }
    let replaced_workspace = workspace_harness.launch();
    assert!(matches!(
        &replaced_workspace.resolution.workspace,
        ResolvedResource::Created(id) if id != &failed_workspace
    ));
    assert_eq!(
        workspace_harness.driver.calls()[0].kind,
        LaunchEffectKind::CreateWorkspace
    );
}

#[tokio::test]
async fn service_grant_failure_blocks_rechecks_and_repairs_the_same_sandbox() {
    let service = ServiceId::new("source-control");
    let harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        LaunchResolution::pending(),
        vec![ServiceRequirement {
            service: service.clone(),
            state: ServiceRequirementState::Satisfied,
        }],
        false,
    );
    harness.run(1).await;
    let sandbox_id = harness.launch().resolution.sandbox.id().unwrap().clone();
    harness.driver.mutate_sandbox(&sandbox_id, |sandbox| {
        sandbox.status = SandboxStatus::Failed;
        sandbox.failure = Some(TypedFailure {
            class: "service_not_granted".into(),
            detail: service.to_string(),
        });
    });
    harness
        .driver
        .set_requirement(service.clone(), ServiceRequirementState::Missing);

    harness.run(2).await;
    assert_eq!(
        harness.launch().blocked_reason,
        Some(LaunchBlockedReason::ConnectionRequired {
            service: service.clone()
        })
    );
    harness.run(3).await;
    assert_eq!(
        harness.launch().blocked_reason,
        Some(LaunchBlockedReason::ConnectionRequired {
            service: service.clone()
        })
    );

    harness
        .driver
        .set_requirement(service, ServiceRequirementState::Satisfied);
    harness.run(4).await;
    let repaired = harness.launch();
    assert_eq!(repaired.blocked_reason, None);
    assert_eq!(repaired.state, LaunchState::SandboxPending);
    assert_eq!(
        harness.driver.sandbox_value(&sandbox_id).status,
        SandboxStatus::Active
    );
    harness.run(5).await;
    assert_eq!(harness.launch().state, LaunchState::WorkspacePending);
    assert_eq!(harness.driver.resource_counts(), (1, 0, 0));
    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].kind, LaunchEffectKind::CreateSandbox);
    assert_eq!(calls[1].kind, LaunchEffectKind::RepairSandboxServiceGrant);
    assert_eq!(calls[0].result_id, calls[1].result_id);
}

#[tokio::test]
async fn orchestration_pause_reblocks_and_resumes_at_the_same_boundary() {
    let harness = Harness::pending();
    harness.driver.set_paused(true);
    harness.run(1).await;
    let blocked = harness.launch();
    assert_eq!(
        blocked.blocked_reason,
        Some(LaunchBlockedReason::OrchestrationPaused)
    );
    assert!(harness.driver.calls().is_empty());

    let revision = blocked.revision;
    harness.run(2).await;
    assert!(harness.launch().revision > revision);
    assert_eq!(
        harness.launch().blocked_reason,
        Some(LaunchBlockedReason::OrchestrationPaused)
    );

    harness.driver.set_paused(false);
    harness.run(3).await;
    let resumed = harness.launch();
    assert_eq!(resumed.blocked_reason, None);
    assert_eq!(resumed.state, LaunchState::SandboxPending);
    assert_eq!(harness.driver.calls().len(), 1);
}

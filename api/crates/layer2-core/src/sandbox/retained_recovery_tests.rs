use super::*;
use crate::sandbox::lifecycle_test_support::{agent, NOW};
use crate::sandbox::runtime_update_test_support::native;
use crate::sandbox::{AgentWorkLease, AgentWorkStep, TypedFailure};

#[path = "retained_recovery_custom_tests.rs"]
mod custom_runtime;

fn snapshot() -> RetainedRecoverySnapshot {
    let mut sandbox = native();
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    sandbox.runtime.as_mut().unwrap().runtime_version = image;
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "private provider detail".into(),
    });
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    let mut child = agent("agt_retained", "original-conversation", 0);
    child.status = AgentStatus::Paused;
    child.conversation.runtime_generation = Some(1);
    RetainedRecoverySnapshot {
        sandbox,
        agents: vec![child],
        workspaces: vec![],
    }
}

#[test]
fn same_image_inspection_is_separate_from_managed_update_eligibility() {
    for explicit in [false, true] {
        let mut held = snapshot();
        if explicit {
            held.sandbox.profile.content.runtime_image = Some(
                held.sandbox
                    .runtime
                    .as_ref()
                    .unwrap()
                    .runtime_version
                    .clone(),
            );
        }
        let bytes = serde_json::to_vec(&held.sandbox).unwrap();
        let source = held.source().unwrap();
        assert_eq!(source.expected_boot, "b".repeat(32));
        assert_eq!(source.generation, 1);
        assert_eq!(source.binding, held.sandbox.runtime.as_ref().unwrap());
        assert_eq!(held.sandbox.supports_retained_runtime_update(), !explicit);
        assert_eq!(bytes, serde_json::to_vec(&held.sandbox).unwrap());
    }
}

#[test]
fn assessment_never_substitutes_image_or_changes_authority() {
    for case in 0..9 {
        let mut held = snapshot();
        match case {
            0 => held.sandbox.profile.content.runtime_image = Some("foreign-image".into()),
            1 => held.sandbox.runtime.as_mut().unwrap().runtime_version = "mutable:tag".into(),
            2 => held.sandbox.runtime.as_mut().unwrap().boot_id = None,
            3 => held.sandbox.runtime.as_mut().unwrap().provision_generation = None,
            4 => held.sandbox.lifecycle.runtime_generation_high_water += 1,
            5 => held.sandbox.lifecycle.home.as_mut().unwrap().released = true,
            6 => held.sandbox.lifecycle.home.as_mut().unwrap().native_pool = None,
            7 => held
                .sandbox
                .work
                .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into()),
            8 => held.sandbox.failure.as_mut().unwrap().class = "terminal_validation".into(),
            _ => unreachable!(),
        }
        let before = held.clone();
        assert!(held.source().is_err(), "case {case}");
        assert_eq!(held, before);
    }
}

#[test]
fn complete_children_do_not_become_recovery_permission() {
    for case in 0..11 {
        let mut held = snapshot();
        let child = &mut held.agents[0];
        match case {
            0 => child.status = AgentStatus::Ready,
            1 => child.status = AgentStatus::Working,
            2 => child.conversation.active_turn = Some(1),
            3 => child.conversation.runtime_generation = Some(2),
            4 => child
                .work
                .schedule(AgentWorkStep::RemoveWorktree, NOW.into()),
            5 => {
                child.work.lease = Some(AgentWorkLease {
                    holder: "worker".into(),
                    fence: 1,
                    expires_at: "2026-09-05T01:00:00Z".into(),
                })
            }
            6 => child.requested_stop_reason = Some(super::super::AgentStopReason::Owner),
            7 => child.conversation.conversation_id = None,
            8 => {
                let duplicate = child.clone();
                held.agents.push(duplicate);
            }
            9 => child.status = AgentStatus::Starting,
            10 => child.status = AgentStatus::Failed,
            _ => unreachable!(),
        }
        assert!(held.source().is_err(), "case {case}");
    }
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        let mut held = snapshot();
        held.agents[0].status = status;
        held.agents[0]
            .work
            .schedule(AgentWorkStep::RemoveWorktree, NOW.into());
        assert!(held.source().is_err(), "terminal children are not skipped");
    }
}

#[test]
fn complete_workspace_evidence_refuses_work_failure_and_foreign_authority() {
    use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
    use crate::sandbox::{
        WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkLease, WorkspaceWorkState,
        WorkspaceWorkStep,
    };
    let mut held = snapshot();
    held.workspaces.push(Workspace {
        id: held.agents[0].workspace_id.clone(),
        sandbox_id: held.sandbox.id.clone(),
        owner: held.sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_retained".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    });
    held.source().unwrap();
    for case in 0..7 {
        let mut changed = held.clone();
        let workspace = &mut changed.workspaces[0];
        match case {
            0 => {
                workspace.status = WorkspaceStatus::Preparing;
                workspace.ready_at = None;
                workspace.resolved_commit = None;
                workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
            }
            1 => {
                workspace.status = WorkspaceStatus::Removing;
                workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
            }
            2 => {
                workspace.work.next_fence = 2;
                workspace.work.lease = Some(WorkspaceWorkLease {
                    holder: "workspace-worker".into(),
                    fence: 1,
                    expires_at: "2026-09-05T01:00:00Z".into(),
                })
            }
            3 => {
                workspace.status = WorkspaceStatus::Failed;
                workspace.failed = Some(TypedFailure {
                    class: "terminal_validation".into(),
                    detail: "fixture".into(),
                });
            }
            4 => workspace.owner = crate::SubjectId("foreign-owner".into()),
            5 => workspace.sandbox_id = crate::SandboxId("sbx_foreign".into()),
            6 => {
                let duplicate = workspace.clone();
                changed.workspaces.push(duplicate);
            }
            _ => unreachable!(),
        }
        let before = changed.clone();
        assert!(changed.source().is_err(), "workspace case {case}");
        assert_eq!(changed, before);
    }
    held.agents.clear();
    held.workspaces[0].status = WorkspaceStatus::Removed;
    held.workspaces[0].removed_at = Some(NOW.into());
    held.source().unwrap();
}

#[test]
fn consumed_one_shot_is_preserved_and_active_lifecycle_is_not_native_health() {
    let mut held = snapshot();
    held.sandbox.lifecycle.unpublished_recovery_attempted = true;
    let before = held.clone();
    assert!(held.source().is_err());
    assert_eq!(held, before);
    assert!(held.sandbox.lifecycle.unpublished_recovery_attempted);
    let mut held = snapshot();
    held.sandbox.status = SandboxStatus::Active;
    held.sandbox.failure = None;
    assert!(matches!(
        held.source(),
        Err(RetainedRecoveryDisposition::ActiveLifecycle)
    ));
}

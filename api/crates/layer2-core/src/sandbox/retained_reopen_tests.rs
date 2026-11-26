use super::*;
use crate::sandbox::lifecycle_test_support::{agent, NOW};

pub(crate) fn fixture() -> RetainedReopenRequest {
    let mut parent = super::runtime_update_test_support::native();
    parent.status = SandboxStatus::Failed;
    parent.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "runtime rejected the lifecycle operation".into(),
    });
    parent.runtime.as_mut().unwrap().runtime_version =
        format!("registry.example/machine@sha256:{}", "a".repeat(64));
    parent.profile.content.runtime_image =
        Some(parent.runtime.as_ref().unwrap().runtime_version.clone());
    parent.runtime.as_mut().unwrap().provision_generation = Some(5);
    parent.lifecycle.runtime_generation_high_water = 5;
    let mut child = agent("agt_retained", "original-conversation", 0);
    child.status = AgentStatus::Paused;
    let workspace = Workspace {
        id: child.workspace_id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: crate::profiles::WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_retained".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: crate::profiles::WorkspaceProfileContent {
                location: crate::profiles::WorkspaceLocation::Directory {
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
        next_worktree_number: 2,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let tree = Worktree {
        id: child.worktree_id.clone(),
        workspace_id: child.workspace_id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(child.id.clone()),
        revision: 1,
    };
    let provider_plan = serde_json::json!({"version":1,"idleReopen":true});
    let proof = RetainedReopenProof {
        expected_runtime: parent.runtime.clone().unwrap(),
        observed_boot: "c".repeat(32),
        home_identity: parent
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap()
            .identity()
            .unwrap(),
        provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&provider_plan)
            .unwrap(),
        provider_plan,
        histories: vec![layer3_adapter::RecoveryConversationHistory {
            agent_id: child.id.to_string(),
            conversation_id: child.conversation.conversation_id.clone().unwrap(),
            history_sha256: "d".repeat(64),
        }],
    };
    RetainedReopenRequest {
        repeat_authorized: false,
        snapshot: RetainedReopenSnapshot {
            command_lanes: vec![(child.id.clone(), Default::default())],
            command_state_sha256: "e".repeat(64),
            event_seq: 0,
            history_state_sha256: "a".repeat(64),
            retained: RetainedRecoverySnapshot {
                sandbox: parent,
                agents: vec![child],
                workspaces: vec![workspace],
            },
            worktrees: vec![tree],
        },
        proof,
        key_sha256: "f".repeat(64),
        requested_at: NOW.into(),
    }
}

#[test]
fn group_d_reopens_only_existing_conversations_without_image_or_generation_change() {
    for explicit in [false, true] {
        for replacement in [false, true] {
            let mut request = fixture();
            if !explicit {
                request
                    .snapshot
                    .retained
                    .sandbox
                    .profile
                    .content
                    .runtime_image = None;
            }
            if !replacement {
                request.proof.observed_boot =
                    request.proof.expected_runtime.boot_id.clone().unwrap();
            }
            let source = &request.snapshot.retained.sandbox;
            let (next, receipt, event) =
                apply_retained_reopen(&request.snapshot, &request).unwrap();
            assert_eq!(next.status, SandboxStatus::Reviving);
            assert!(matches!(
                next.lifecycle.operation,
                Some(SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::ConversationReopen,
                    ..
                })
            ));
            assert_eq!(
                next.runtime.as_ref().unwrap().runtime_version,
                source.runtime.as_ref().unwrap().runtime_version
            );
            assert_eq!(next.runtime.as_ref().unwrap().provision_generation, Some(5));
            assert_eq!(next.lifecycle.runtime_generation_high_water, 5);
            assert_eq!(next.lifecycle.home, source.lifecycle.home);
            assert_eq!(next.profile, source.profile);
            assert_eq!(next.credential, source.credential);
            assert!(next.lifecycle.runtime_provision.is_none());
            assert!(next.lifecycle.suspended_runtime.is_none());
            assert!(next.lifecycle.runtime_update.is_none());
            assert_eq!(
                receipt.proof.expected_runtime,
                source.runtime.clone().unwrap()
            );
            assert_eq!(receipt.original_failure, source.failure.clone().unwrap());
            assert_eq!(receipt.accepted.revision, source.revision + 1);
            assert_eq!(
                event,
                SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Failed,
                    to: SandboxStatus::Reviving
                }
            );
            if replacement {
                assert!(
                    source.validate_runtime_generation_successor(&next).is_err(),
                    "generic boot rewrites remain forbidden"
                );
            }
            let decoded: Sandbox =
                serde_json::from_slice(&serde_json::to_vec(&next).unwrap()).unwrap();
            decoded.validate().unwrap();
        }
    }
}

#[test]
fn retained_reopen_rejects_unproved_history_and_changed_authority() {
    for case in 0..14 {
        let mut request = fixture();
        match case {
            0 => request.proof.observed_boot.clear(),
            1 => request.proof.expected_runtime.boot_id = Some("d".repeat(32)),
            2 => request.proof.home_identity = "foreign-home".into(),
            3 => request.proof.provider_plan_sha256 = "0".repeat(64),
            4 => request.proof.histories.clear(),
            5 => request.proof.histories[0].conversation_id = "foreign-conversation".into(),
            6 => {
                request.snapshot.retained.agents[0]
                    .conversation
                    .runtime_generation = Some(5)
            }
            7 => request.snapshot.retained.agents[0].status = AgentStatus::Working,
            8 => {
                request.snapshot.retained.agents[0].requested_stop_reason =
                    Some(AgentStopReason::Owner)
            }
            9 => request.snapshot.worktrees[0].status = WorktreeStatus::Removing,
            10 => request.snapshot.retained.sandbox.credential.delivered = false,
            11 => {
                request
                    .snapshot
                    .retained
                    .sandbox
                    .lifecycle
                    .runtime_generation_high_water += 1
            }
            12 => request.snapshot.command_lanes.clear(),
            13 => request.snapshot.retained.agents.clear(),
            _ => unreachable!(),
        }
        assert!(
            apply_retained_reopen(&request.snapshot, &request).is_err(),
            "case {case}"
        );
    }
    let request = fixture();
    let mut current = request.snapshot.clone();
    current.retained.agents[0].conversation.last_seq += 1;
    assert!(apply_retained_reopen(&current, &request).is_err());
    current = request.snapshot.clone();
    current.event_seq += 1;
    assert!(apply_retained_reopen(&current, &request).is_err());
}

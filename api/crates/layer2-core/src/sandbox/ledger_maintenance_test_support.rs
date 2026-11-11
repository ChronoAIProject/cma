use super::super::ledger_maintenance_plan::*;
use super::super::{lifecycle_test_support, runtime_update_test_support};
use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use crate::sandbox::*;
use crate::{WorkspaceId, WorkspaceProfileId, WorktreeId};
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};
const NOW: &str = "2026-09-05T00:30:02Z";

pub(crate) fn fixture() -> LedgerMaintenancePlanV1 {
    let sandbox = runtime_update_test_support::native();
    let mut agent = lifecycle_test_support::agent("fixture-agent", "retained-conversation", 1);
    let workspace = Workspace {
        id: WorkspaceId("fixture-workspace".into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("fixture-profile".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "fixture".into(),
                },
                name: "fixture".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "fixture".into(),
        path: "/home/cma/fixture".into(),
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
    let worktree = Worktree {
        id: WorktreeId("fixture-worktree".into()),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/fixture/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent.id.clone()),
        revision: 1,
    };
    agent.workspace_id = workspace.id.clone();
    agent.worktree_id = worktree.id.clone();
    let binding = LedgerBindingV1 {
        format: LEDGER_BINDING_FORMAT.into(),
        sandbox_id: sandbox.id.to_string(),
        home_identity: sandbox
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap()
            .identity()
            .unwrap(),
        generation: 1,
        boot_id: "b".repeat(32),
        roots_sha256: "a".repeat(64),
        source_format: LedgerSourceFormatV1::V1,
        archive_id: uuid::Uuid::from_u128(1).to_string(),
        maintenance_id: "fixture-operation".into(),
        maintenance_epoch: 1,
        sandbox_revision: sandbox.revision,
        candidate_sha256: "c".repeat(64),
        recovery_sha256: "d".repeat(64),
        exclusion_receipt_sha256: "e".repeat(64),
    };
    let lane = LedgerMaintenanceLaneV1 {
        agent_id: agent.id.clone(),
        lane: Some(crate::sandbox::agent_io::CommandLane {
            last_order: 1,
            next_fence: 2,
            lease: None,
        }),
    };
    LedgerMaintenancePlanV1 {
        format: "ledger-maintenance-plan-v1".into(),
        binding,
        source: LedgerMaintenanceSnapshotV1 {
            source_documents_sha256: "a".repeat(64),
            sandbox,
            sandbox_event_head: 1,
            agents: vec![agent],
            workspaces: vec![workspace],
            worktrees: vec![worktree],
            command_lanes: vec![lane],
        },
        consent_receipt_sha256: "f".repeat(64),
        requested_at: lifecycle_test_support::NOW.into(),
    }
}

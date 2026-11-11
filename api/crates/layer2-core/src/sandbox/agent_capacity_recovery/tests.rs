use super::*;
use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use crate::sandbox::lifecycle_test_support;
use crate::sandbox::{
    AgentCleanupDebt, AgentStopReason, AgentWorkLease, AgentWorkReceipt, FirstMessageRecord,
    SandboxHome, TypedFailure, WorkspaceOrigin, WorkspaceSource, WorkspaceWorkState,
};
use crate::WorkspaceProfileId;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

const NOW: &str = lifecycle_test_support::NOW;
const LATER: &str = "2026-09-05T00:31:01Z";

mod plan;

struct Fixture {
    agent: Agent,
    sandbox: Sandbox,
    workspace: Workspace,
    worktree: Worktree,
}

impl Fixture {
    fn new() -> Self {
        let mut agent =
            lifecycle_test_support::agent("capacity-fixture", "original-native", 10_001);
        agent.status = AgentStatus::Failed;
        agent.failure = Some(TypedFailure {
            class: AGENT_LOG_CAPACITY_FAILURE.into(),
            detail: "retained capacity failure".into(),
        });
        agent.work.next_fence = 8;
        agent.work.receipts.push(AgentWorkReceipt {
            step: AgentWorkStep::OpenConversation,
            operation_id: agent.operation_id(AgentWorkStep::OpenConversation),
            request_sha256: "e".repeat(64),
            completed_at: agent.created_at.clone(),
        });
        let mut sandbox = lifecycle_test_support::sandbox();
        let runtime = sandbox.runtime.as_mut().unwrap();
        runtime.boot_id = Some("a".repeat(32));
        runtime.provision_generation = Some(4);
        sandbox.lifecycle.runtime_generation_high_water = 4;
        let mut home = SandboxHome::reserve(
            &sandbox.id,
            HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "empty".into(),
                template_generation: "seed".into(),
            },
        )
        .unwrap();
        home.bind(HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.request.volume_id.clone(),
            request_sha256: home.request.computed_sha256(),
            storage_reference: "original-home".into(),
            storage_generation: "original".into(),
        })
        .unwrap();
        sandbox.lifecycle.home = Some(Box::new(home));
        let workspace = Workspace {
            id: agent.workspace_id.clone(),
            sandbox_id: sandbox.id.clone(),
            owner: agent.owner.clone(),
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: WorkspaceProfileId("workspace-profile".into()),
                revision: 1,
                digest: "f".repeat(64),
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
            resolved_commit: Some("a".repeat(40)),
            status: WorkspaceStatus::Ready,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
            next_worktree_number: 2,
            created_at: agent.created_at.clone(),
            ready_at: Some(NOW.into()),
            failed: None,
            removed_at: None,
            revision: 3,
        };
        let worktree = Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: workspace.id.clone(),
            sandbox_id: sandbox.id.clone(),
            owner: agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/project/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: workspace.resolved_commit.clone().unwrap(),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 2,
        };
        Self {
            agent,
            sandbox,
            workspace,
            worktree,
        }
    }

    fn context(&self) -> CapacityRecoveryContext<'_> {
        CapacityRecoveryContext {
            agent: &self.agent,
            sandbox: &self.sandbox,
            workspace: &self.workspace,
            worktree: &self.worktree,
        }
    }

    fn classify(&self) -> CapacityRecoveryDisposition {
        self.context().classify(NOW)
    }
}

#[test]
fn capacity_failure_is_only_a_candidate_and_preflight_never_changes_state() {
    let fixture = Fixture::new();
    let original = serde_json::to_value((
        &fixture.agent,
        &fixture.sandbox,
        &fixture.workspace,
        &fixture.worktree,
    ))
    .unwrap();
    assert_eq!(
        fixture.classify(),
        CapacityRecoveryDisposition::EvidenceRequired
    );
    assert_eq!(
        original,
        serde_json::to_value((
            &fixture.agent,
            &fixture.sandbox,
            &fixture.workspace,
            &fixture.worktree
        ))
        .unwrap()
    );
    assert_eq!(fixture.agent.status, AgentStatus::Failed);
    assert_eq!(fixture.agent.conversation.last_seq, 10_001);
}

#[test]
fn nine_incident_parent_dispositions_do_not_imply_nine_recovered_agents() {
    use CapacityRecoveryDisposition as D;
    let parents = [
        SandboxStatus::Failed,
        SandboxStatus::Failed,
        SandboxStatus::Failed,
        SandboxStatus::Failed,
        SandboxStatus::Failed,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retiring,
        SandboxStatus::CredentialRotating,
    ];
    let dispositions: Vec<_> = parents
        .into_iter()
        .map(|status| {
            let mut fixture = Fixture::new();
            fixture.sandbox.status = status;
            fixture.classify()
        })
        .collect();
    assert_eq!(
        dispositions,
        [
            D::ParentRecoveryRequired,
            D::ParentRecoveryRequired,
            D::ParentRecoveryRequired,
            D::ParentRecoveryRequired,
            D::ParentRecoveryRequired,
            D::ParentRecoveryRequired,
            D::CleanupOnly,
            D::CleanupOnly,
            D::ParentNotReady
        ]
    );
}

#[test]
fn cleanup_and_terminal_intent_win_even_when_parent_is_failed() {
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        let mut f = Fixture::new();
        f.agent.status = status;
        f.sandbox.status = SandboxStatus::Failed;
        assert_eq!(f.classify(), CapacityRecoveryDisposition::TerminalPreserved);
    }
    for step in [
        AgentWorkStep::FenceInteraction,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ] {
        let mut f = Fixture::new();
        f.sandbox.status = SandboxStatus::Failed;
        f.agent.work.step = step;
        assert_eq!(f.classify(), CapacityRecoveryDisposition::CleanupOnly);
    }
    let mut f = Fixture::new();
    f.agent.requested_stop_reason = Some(AgentStopReason::Owner);
    assert_eq!(f.classify(), CapacityRecoveryDisposition::CleanupOnly);
    let mut f = Fixture::new();
    f.workspace.status = WorkspaceStatus::Removing;
    assert_eq!(f.classify(), CapacityRecoveryDisposition::CleanupOnly);
    let mut f = Fixture::new();
    f.worktree.status = WorktreeStatus::Removed;
    assert_eq!(f.classify(), CapacityRecoveryDisposition::CleanupOnly);
}

#[test]
fn valid_old_lease_waits_expiry_and_malformed_lease_never_expires() {
    let mut f = Fixture::new();
    f.agent.work.lease = Some(AgentWorkLease {
        holder: "old-worker".into(),
        fence: 7,
        expires_at: LATER.into(),
    });
    assert_eq!(f.classify(), CapacityRecoveryDisposition::LeaseActive);
    assert_eq!(
        f.context().classify(LATER),
        CapacityRecoveryDisposition::EvidenceRequired
    );
    f.agent.work.lease.as_mut().unwrap().expires_at = "invalid".into();
    assert_eq!(f.classify(), CapacityRecoveryDisposition::InvalidState);
    f.agent.work.lease.as_mut().unwrap().expires_at = NOW.into();
    f.agent.work.lease.as_mut().unwrap().fence = 8;
    assert_eq!(f.classify(), CapacityRecoveryDisposition::InvalidState);
}

#[test]
fn unrelated_failure_and_new_work_never_get_capacity_permission() {
    let mut f = Fixture::new();
    f.agent.failure.as_mut().unwrap().class = "runtime_lost".into();
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::NotCapacityFailure
    );
    let mut f = Fixture::new();
    f.agent.work.step = AgentWorkStep::DispatchFirstMessage;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::NotCapacityFailure
    );
    let mut f = Fixture::new();
    f.agent.status = AgentStatus::Ready;
    f.agent.failure = None;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::NotCapacityFailure
    );
    f.agent.work.step = AgentWorkStep::PublishConnection;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::ContinueOriginalWork
    );
}

#[test]
fn lineage_is_checked_before_parent_disposition() {
    let mut f = Fixture::new();
    f.sandbox.status = SandboxStatus::Retiring;
    f.sandbox.owner = crate::SubjectId::new("different-owner");
    assert_eq!(f.classify(), CapacityRecoveryDisposition::LineageMismatch);
    let mut f = Fixture::new();
    f.worktree.agent_id = None;
    assert_eq!(f.classify(), CapacityRecoveryDisposition::LineageMismatch);
}

#[test]
fn missing_duplicate_or_changed_open_receipt_is_not_replayed() {
    let mut f = Fixture::new();
    f.agent.work.receipts.clear();
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::OpenReceiptRequired
    );
    let mut f = Fixture::new();
    f.agent.work.receipts.push(f.agent.work.receipts[0].clone());
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::OpenReceiptRequired
    );
    let mut f = Fixture::new();
    f.agent.work.receipts[0]
        .operation_id
        .push_str("-replacement");
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::OpenReceiptRequired
    );
    let mut f = Fixture::new();
    f.agent.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: "original".into(),
        request_sha256: "a".repeat(64),
    });
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::EffectReconciliationRequired
    );
}

#[test]
fn first_message_outcome_survives_repeated_dry_runs_and_restart() {
    for phase in 0..4 {
        let mut f = Fixture::new();
        let mut first =
            FirstMessageRecord::new(&f.agent.creation_key, "fixture input", NOW).unwrap();
        match phase {
            1 => first.fail_startup(NOW.into()).unwrap(),
            2 | 3 => {
                first.fence_dispatch_for(5, 6, 7, NOW.into()).unwrap();
                if phase == 3 {
                    first.mark_write_confirmed(NOW.into()).unwrap();
                }
                f.agent.conversation.active_turn = Some(5);
            }
            _ => {}
        }
        f.agent.conversation.first_message = Some(first.clone());
        for _ in 0..3 {
            assert_eq!(f.classify(), CapacityRecoveryDisposition::EvidenceRequired);
            f.agent = serde_json::from_slice(&serde_json::to_vec(&f.agent).unwrap()).unwrap();
        }
        assert_eq!(f.agent.conversation.first_message, Some(first));
        assert_eq!(f.agent.work.step, AgentWorkStep::None);
    }
}

#[test]
fn retained_native_and_home_authority_are_required() {
    let mut f = Fixture::new();
    f.agent.conversation.conversation_id = None;
    f.agent.conversation.runtime_generation = None;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::NativeIdentityRequired
    );
    let mut f = Fixture::new();
    f.sandbox.runtime.as_mut().unwrap().provision_generation = Some(3);
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::NativeIdentityRequired
    );
    let mut f = Fixture::new();
    f.sandbox.lifecycle.home = None;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::HomeIdentityRequired
    );
    let mut f = Fixture::new();
    f.sandbox.lifecycle.home.as_mut().unwrap().released = true;
    assert_eq!(
        f.classify(),
        CapacityRecoveryDisposition::HomeIdentityRequired
    );
}

#[test]
fn transition_and_parent_work_are_never_superseded_by_capacity_recovery() {
    for status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
    ] {
        let mut f = Fixture::new();
        f.sandbox.status = status;
        assert_eq!(f.classify(), CapacityRecoveryDisposition::ParentNotReady);
    }
    let mut f = Fixture::new();
    f.workspace.work.step = WorkspaceWorkStep::Remove;
    assert_eq!(f.classify(), CapacityRecoveryDisposition::ParentNotReady);
}

#[test]
fn clocks_and_overflow_do_not_create_recovery_authority() {
    let f = Fixture::new();
    assert_eq!(
        f.context().classify("invalid"),
        CapacityRecoveryDisposition::InvalidState
    );
    let mut f = Fixture::new();
    f.agent.updated_at = LATER.into();
    assert_eq!(f.classify(), CapacityRecoveryDisposition::InvalidState);
    for field in [0, 1, 2] {
        let mut f = Fixture::new();
        match field {
            0 => f.agent.revision = u64::MAX,
            1 => f.agent.work.next_fence = u64::MAX,
            _ => f.agent.conversation.last_seq = u64::MAX,
        }
        assert_eq!(f.classify(), CapacityRecoveryDisposition::InvalidState);
    }
}

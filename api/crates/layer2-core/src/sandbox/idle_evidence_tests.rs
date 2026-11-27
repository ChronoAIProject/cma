use super::agentless_idle_refusal;
use crate::ids::{SandboxId, SubjectId, WorkspaceId, WorkspaceProfileId};
use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use crate::sandbox::lifecycle_test_support::{agent, sandbox, IDLE_AT, NOW};
use crate::sandbox::{
    Agent, AgentStatus, AgentWorkLease, AgentWorkStep, InactivityPauseRefusal as Refusal,
    QueuedAgentMessage, Sandbox, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkLease, WorkspaceWorkState, WorkspaceWorkStep,
};

const THRESHOLD: u64 = 900;

#[test]
fn parent_time_and_runtime_evidence_must_be_complete_and_old() {
    let sandbox = sandbox();
    assert_eq!(
        agentless_idle_refusal(&sandbox, &[], &[], NOW, THRESHOLD),
        None
    );
    assert_eq!(
        agentless_idle_refusal(&sandbox, &[], &[], "invalid", THRESHOLD),
        Some(Refusal::InvalidActivityTime)
    );

    let mut recent = sandbox.clone();
    recent.created_at = NOW.into();
    assert_eq!(
        agentless_idle_refusal(&recent, &[], &[], NOW, THRESHOLD),
        Some(Refusal::RecentSandboxActivity)
    );

    recent = sandbox.clone();
    recent.last_activity_at = NOW.into();
    assert_eq!(
        agentless_idle_refusal(&recent, &[], &[], NOW, THRESHOLD),
        Some(Refusal::RecentSandboxActivity)
    );

    let mut no_runtime = sandbox.clone();
    no_runtime.runtime = None;
    assert_eq!(
        agentless_idle_refusal(&no_runtime, &[], &[], NOW, THRESHOLD),
        Some(Refusal::InvalidRuntime)
    );

    recent = sandbox;
    recent.runtime.as_mut().unwrap().bound_at = NOW.into();
    assert_eq!(
        agentless_idle_refusal(&recent, &[], &[], NOW, THRESHOLD),
        Some(Refusal::RecentRuntimeActivity)
    );
}

#[test]
fn retained_agent_must_have_matching_ownership_and_no_pending_activity() {
    let sandbox = sandbox();
    let mut terminal = agent("agt_terminal", "conversation-terminal", 1);
    terminal.status = AgentStatus::Stopped;
    let refuse = |value: &Agent| {
        agentless_idle_refusal(&sandbox, std::slice::from_ref(value), &[], NOW, THRESHOLD)
    };
    assert_eq!(refuse(&terminal), None);

    let identity_cases: [fn(&mut Agent); 3] = [
        |agent| agent.sandbox_id = SandboxId("sbx_other".into()),
        |agent| agent.owner = SubjectId("other".into()),
        |agent| agent.status = AgentStatus::Ready,
    ];
    for change in identity_cases {
        let mut invalid = terminal.clone();
        change(&mut invalid);
        assert_eq!(refuse(&invalid), Some(Refusal::InvalidAgentSet));
    }

    let activity_cases: [fn(&mut Agent); 4] = [
        |agent| agent.work.step = AgentWorkStep::EnsureProcessClosed,
        |agent| {
            agent.work.lease = Some(AgentWorkLease {
                holder: "worker".into(),
                fence: 1,
                expires_at: NOW.into(),
            });
        },
        |agent| agent.conversation.active_turn = Some(2),
        |agent| {
            agent.conversation.queued_messages =
                vec![QueuedAgentMessage::new(2, "queued".into(), IDLE_AT.into()).unwrap()];
        },
    ];
    for change in activity_cases {
        let mut busy = terminal.clone();
        change(&mut busy);
        assert_eq!(refuse(&busy), Some(Refusal::PendingAgentWork));
    }

    terminal.updated_at = NOW.into();
    assert_eq!(refuse(&terminal), Some(Refusal::RecentAgentActivity));
}

#[test]
fn retained_workspace_must_have_matching_ownership_and_old_completion() {
    let sandbox = sandbox();
    let ready = workspace(&sandbox);
    let refuse = |value: &Workspace| {
        agentless_idle_refusal(&sandbox, &[], std::slice::from_ref(value), NOW, THRESHOLD)
    };
    assert_eq!(refuse(&ready), None);

    let mut removed = ready.clone();
    removed.status = WorkspaceStatus::Removed;
    removed.ready_at = None;
    removed.removed_at = Some(IDLE_AT.into());
    assert_eq!(refuse(&removed), None);
    removed.removed_at = None;
    assert_eq!(refuse(&removed), Some(Refusal::InvalidActivityTime));

    let identity_cases: [fn(&mut Workspace); 2] = [
        |workspace| workspace.sandbox_id = SandboxId("sbx_other".into()),
        |workspace| workspace.owner = SubjectId("other".into()),
    ];
    for change in identity_cases {
        let mut invalid = ready.clone();
        change(&mut invalid);
        assert_eq!(refuse(&invalid), Some(Refusal::InvalidStoredEvidence));
    }

    let work_cases: [fn(&mut Workspace); 3] = [
        |workspace| workspace.status = WorkspaceStatus::Preparing,
        |workspace| workspace.work.step = WorkspaceWorkStep::Prepare,
        |workspace| {
            workspace.work.lease = Some(WorkspaceWorkLease {
                holder: "worker".into(),
                fence: 1,
                expires_at: NOW.into(),
            });
        },
    ];
    for change in work_cases {
        let mut busy = ready.clone();
        change(&mut busy);
        assert_eq!(refuse(&busy), Some(Refusal::PendingWorkspaceWork));
    }

    let mut unsettled = ready.clone();
    unsettled.ready_at = None;
    assert_eq!(refuse(&unsettled), Some(Refusal::InvalidActivityTime));
    unsettled.ready_at = Some(NOW.into());
    assert_eq!(refuse(&unsettled), Some(Refusal::RecentWorkspaceActivity));
}

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace {
        id: WorkspaceId("wks_idle".into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_idle".into()),
            revision: 1,
            digest: "b".repeat(64),
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
        resolved_commit: None,
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, IDLE_AT.into()),
        next_worktree_number: 1,
        created_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

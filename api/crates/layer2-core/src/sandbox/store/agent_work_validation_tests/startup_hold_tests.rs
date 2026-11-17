use super::*;
use crate::sandbox::store::{
    agent_cleanup_capacity_retry, plan_agent_repeated_failure,
    plan_agent_runtime_ledger_hold_retry, plan_agent_runtime_startup_hold_retry,
    validate_agent_cleanup_capacity_log, AGENT_RUNTIME_STARTUP_HOLD,
    AGENT_RUNTIME_STARTUP_HOLD_DETAIL,
};
use crate::sandbox::AgentLogEntry;

#[test]
fn startup_hold_keeps_each_supported_step_pending_and_reuses_only_its_exact_tail() {
    for step in [
        AgentWorkStep::AddWorktree,
        AgentWorkStep::MaterializeHome,
        AgentWorkStep::OpenConversation,
        AgentWorkStep::PublishReady,
        AgentWorkStep::DispatchFirstMessage,
        AgentWorkStep::PublishConnection,
        AgentWorkStep::InstallSkill,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::RemoveHome,
    ] {
        let status = if step == AgentWorkStep::RemoveHome {
            AgentStatus::Stopped
        } else {
            AgentStatus::Starting
        };
        let (mut current, _) = claimed(step, status, WorktreeStatus::Creating);
        if step == AgentWorkStep::CloseProcess {
            current.requested_stop_reason = Some(AgentStopReason::Owner);
        }
        current.conversation.last_seq = 10_000;
        let plan = |last| {
            plan_agent_runtime_startup_hold_retry(
                &current,
                last,
                "worker",
                fence(&current),
                AT,
                RETRY_AT,
            )
            .unwrap()
        };
        let first = plan(None);
        assert!(agent_cleanup_capacity_retry(&current, &first));
        assert_eq!(first.agent.status, current.status);
        assert_eq!(first.agent.work.step, step);
        assert_eq!(first.agent.work.attempts, current.work.attempts + 1);
        assert_eq!(first.agent.conversation.last_seq, 10_001);
        assert!(
            matches!(&first.agent_logs[..], [AgentLogKind::WorkStepFailed { failure, .. }]
            if failure.class == AGENT_RUNTIME_STARTUP_HOLD && failure.detail == AGENT_RUNTIME_STARTUP_HOLD_DETAIL)
        );
        validate_agent_cleanup_capacity_log(&current, &first, None).unwrap();
        let tail = AgentLogEntry {
            agent_id: current.id.clone(),
            seq: current.conversation.last_seq,
            appended_at: NOW.into(),
            kind: first.agent_logs[0].clone(),
        };
        let retry = plan(Some(&tail));
        assert!(retry.agent_logs.is_empty());
        assert_eq!(retry.agent.conversation.last_seq, 10_000);
        validate_agent_cleanup_capacity_log(&current, &retry, Some(&tail)).unwrap();
        // Skill preparation also permits ordinary metadata-only retries.
        if step != AgentWorkStep::InstallSkill {
            assert!(validate_agent_cleanup_capacity_log(&current, &retry, None).is_err());
        }
        let mut wrong = tail.clone();
        wrong.seq -= 1;
        assert_eq!(plan(Some(&wrong)).agent_logs.len(), 1);
        if step != AgentWorkStep::InstallSkill {
            assert!(validate_agent_cleanup_capacity_log(&current, &retry, Some(&wrong)).is_err());
        }
        assert!(plan_agent_repeated_failure(
            &current,
            Some(&tail),
            AGENT_RUNTIME_STARTUP_HOLD,
            "worker",
            fence(&current),
            AT,
            RETRY_AT,
        )
        .unwrap()
        .is_none());
    }
}

#[test]
fn changing_hold_class_appends_one_marker_in_both_directions() {
    let (current, _) = claimed(
        AgentWorkStep::OpenConversation,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    type Planner = fn(
        &Agent,
        Option<&AgentLogEntry>,
        &str,
        u64,
        &str,
        &str,
    ) -> Result<AgentWorkMutation, SandboxStoreError>;
    let planners: [(Planner, Planner); 2] = [
        (
            plan_agent_runtime_ledger_hold_retry,
            plan_agent_runtime_startup_hold_retry,
        ),
        (
            plan_agent_runtime_startup_hold_retry,
            plan_agent_runtime_ledger_hold_retry,
        ),
    ];
    for (previous, next) in planners {
        let first = previous(&current, None, "worker", fence(&current), AT, RETRY_AT).unwrap();
        let tail = AgentLogEntry {
            agent_id: current.id.clone(),
            seq: current.conversation.last_seq,
            appended_at: NOW.into(),
            kind: first.agent_logs[0].clone(),
        };
        let changed = next(
            &current,
            Some(&tail),
            "worker",
            fence(&current),
            AT,
            RETRY_AT,
        )
        .unwrap();
        assert_eq!(changed.agent_logs.len(), 1);
        assert_ne!(changed.agent_logs, first.agent_logs);
        validate_agent_cleanup_capacity_log(&current, &changed, Some(&tail)).unwrap();
        let retained = previous(
            &current,
            Some(&tail),
            "worker",
            fence(&current),
            AT,
            RETRY_AT,
        )
        .unwrap();
        assert!(retained.agent_logs.is_empty());
        validate_agent_cleanup_capacity_log(&current, &retained, Some(&tail)).unwrap();
    }
}

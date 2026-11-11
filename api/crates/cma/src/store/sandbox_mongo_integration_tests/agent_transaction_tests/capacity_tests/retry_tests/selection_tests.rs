use super::*;
use layer2_core::sandbox::store::{
    agent_cleanup_capacity_retry_hint, plan_agent_marked_cleanup_capacity_retry,
    NORMAL_AGENT_LOG_ENTRIES,
};

fn selected(
    agent: &Agent,
    tail: Option<&AgentLogEntry>,
) -> Result<Option<AgentWorkMutation>, SandboxStoreError> {
    plan_agent_marked_cleanup_capacity_retry(
        agent,
        tail,
        "worker",
        agent.work.lease.as_ref().unwrap().fence,
        MINUTE,
        TWO_MINUTES,
    )
}

#[test]
fn marked_selection_matches_known_abort_settlement_for_every_cleanup_step() {
    for step in [
        AgentWorkStep::FenceInteraction,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ] {
        let before = fixture(step);
        assert!(!agent_cleanup_capacity_retry_hint(&before));
        assert!(selected(&before, None).unwrap().is_none());
        let first = plan(&before, None);
        let tail = marker(&first);
        let mut current = first.agent;
        current
            .work
            .claim("worker".into(), MINUTE, TWO_MINUTES.into())
            .unwrap();
        current.revision += 1;
        assert!(agent_cleanup_capacity_retry_hint(&current));
        let direct = selected(&current, Some(&tail)).unwrap().unwrap();
        let control = plan_agent_cleanup_capacity_retry(
            &current,
            Some(&tail),
            "worker",
            current.work.lease.as_ref().unwrap().fence,
            MINUTE,
            TWO_MINUTES,
        )
        .unwrap();
        assert_eq!(direct.agent, control.agent);
        assert_eq!(direct.agent_logs, control.agent_logs);
        assert_eq!(
            direct.expected_agent_revision,
            control.expected_agent_revision
        );
        assert!(direct.agent_logs.is_empty());
        assert_eq!(direct.agent.work.attempts, current.work.attempts + 1);
        assert_eq!(
            direct.agent.conversation.last_seq,
            current.conversation.last_seq
        );
        assert_eq!(direct.agent.status, current.status);
        assert_eq!(direct.agent.work.step, current.work.step);
        assert_eq!(direct.agent.work.ready_at, TWO_MINUTES);
        validate_agent_cleanup_capacity_log(&current, &direct, Some(&tail)).unwrap();
    }
}

#[test]
fn selection_requires_the_exact_marker_and_an_exhausted_entry_head() {
    let first = plan(&fixture(AgentWorkStep::CloseProcess), None);
    let tail = marker(&first);
    let mut current = first.agent;
    current
        .work
        .claim("worker".into(), MINUTE, TWO_MINUTES.into())
        .unwrap();
    current.revision += 1;
    assert!(selected(&current, None).unwrap().is_none());
    for variant in 0..8 {
        let mut agent = current.clone();
        let mut last = tail.clone();
        match variant {
            0 => {
                agent.conversation.last_seq = NORMAL_AGENT_LOG_ENTRIES;
                last.seq = NORMAL_AGENT_LOG_ENTRIES;
            }
            1 => {
                agent.conversation.last_seq = 30;
                last.seq = 30;
            } // byte-only capacity is unchanged
            2 => last.agent_id = AgentId("agt_other".into()),
            3 => last.seq -= 1,
            4 => agent.work.step = AgentWorkStep::PublishConnection,
            5 => agent.requested_stop_reason = None,
            6 => {
                if let AgentLogKind::WorkStepFailed { step, .. } = &mut last.kind {
                    *step = AgentWorkStep::RemoveHome;
                }
            }
            _ => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut last.kind {
                    failure.detail.push('!');
                }
            }
        }
        assert!(
            selected(&agent, Some(&last)).unwrap().is_none(),
            "variant {variant}"
        );
    }
    let fence = current.work.lease.as_ref().unwrap().fence;
    for (holder, fence, at, due) in [
        ("other", fence, MINUTE, TWO_MINUTES),
        ("worker", fence + 1, MINUTE, TWO_MINUTES),
        ("worker", fence, TWO_MINUTES, "2026-09-05T00:03:00Z"),
    ] {
        assert!(matches!(
            plan_agent_marked_cleanup_capacity_retry(&current, Some(&tail), holder, fence, at, due),
            Err(SandboxStoreError::Conflict)
        ));
    }
}

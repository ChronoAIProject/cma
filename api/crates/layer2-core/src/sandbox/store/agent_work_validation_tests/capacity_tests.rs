use super::*;
use crate::sandbox::store::{plan_agent_log_capacity_failure, AGENT_LOG_CAPACITY_FAILURE};
use crate::sandbox::FirstMessageDelivery;

#[test]
fn capacity_settlement_preserves_skill_selection_and_fails_only_pending_installation() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::InstallSkill,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut current);
    current
        .work
        .live_skills
        .push(crate::sandbox::AgentSkillInstallation::new(
            crate::profiles::SkillRef {
                source: crate::profiles::SkillSource::Ornn,
                id: crate::SkillId::new("research"),
                version: Some("1.0".into()),
            },
        ));
    let mutation =
        plan_agent_log_capacity_failure(&current, &worktree, "worker", fence(&current), AT)
            .unwrap();
    assert_valid(&current, &worktree, &mutation);
    let skill = &mutation.agent.work.live_skills[0];
    assert_eq!(skill.reference, current.work.live_skills[0].reference);
    assert_eq!(
        skill.status,
        crate::sandbox::AgentSkillInstallStatus::Failed
    );
    assert_eq!(skill.error.as_deref(), Some(AGENT_LOG_CAPACITY_FAILURE));
    let mut forged = mutation.clone();
    forged.agent.work.live_skills[0].reference.version = Some("2.0".into());
    assert_conflict(&current, &worktree, &forged);
}

#[test]
fn capacity_settlement_is_exact_and_preserves_ambiguous_input_authority() {
    for dispatched in [false, true] {
        let (mut current, worktree) = claimed(
            AgentWorkStep::PublishConnection,
            AgentStatus::Ready,
            WorktreeStatus::Ready,
        );
        add_startup_receipts(&mut current);
        current.conversation.last_seq = 10_000;
        let mut first = FirstMessageRecord::new("capacity", "Retain this input", NOW).unwrap();
        if dispatched {
            first.fence_dispatch(9_990, 2, NOW.into()).unwrap();
            first
                .require_recovery(
                    crate::sandbox::FirstMessageFailure::new(
                        "ambiguous_write",
                        "Delivery requires original receipt recovery",
                        false,
                        NOW,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        current.conversation.first_message = Some(first.clone());
        let mutation =
            plan_agent_log_capacity_failure(&current, &worktree, "worker", fence(&current), AT)
                .unwrap();
        assert_valid(&current, &worktree, &mutation);
        let settled = mutation.agent.conversation.first_message.as_ref().unwrap();
        if dispatched {
            assert_eq!(settled, &first);
        } else {
            assert_eq!(settled.state, FirstMessageDelivery::Failed);
            assert_eq!(
                settled.failure.as_ref().unwrap().code,
                "agent_startup_failed"
            );
            assert!(!settled.failure.as_ref().unwrap().retryable);
            assert_eq!(settled.text, first.text);
            assert_eq!(settled.payload_sha256, first.payload_sha256);
            assert_eq!(settled.attempt, 0);
        }
        assert_eq!(
            mutation.agent.failure.as_ref().unwrap().class,
            AGENT_LOG_CAPACITY_FAILURE
        );
        for variant in 0..11 {
            let mut invalid = mutation.clone();
            match variant {
                0 => invalid.holder = "replacement".into(),
                1 => invalid.fence += 1,
                2 => invalid.agent.work.receipts.clear(),
                3 => invalid.agent.conversation.first_message = None,
                4 => invalid.agent.conversation.runtime_generation = Some(99),
                5 => invalid.agent.failure.as_mut().unwrap().class = "ordinary_failure".into(),
                6 => invalid.agent.failure.as_mut().unwrap().detail.push('!'),
                7 => invalid.sandbox_event = None,
                8 => invalid.agent_logs.clear(),
                9 => invalid.agent.stopped_at = Some(AT.into()),
                _ => invalid.agent.work.ready_at = RETRY_AT.into(),
            }
            assert_conflict(&current, &worktree, &invalid);
        }
    }
}

#[test]
fn capacity_failure_does_not_relax_the_ordinary_transition_or_cleanup_graph() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::PublishConnection,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut current);
    let mutation =
        plan_agent_log_capacity_failure(&current, &worktree, "worker", fence(&current), AT)
            .unwrap();
    assert_valid(&current, &worktree, &mutation);
    let mut expired = current.clone();
    expired.work.lease.as_mut().unwrap().expires_at = AT.into();
    assert_conflict(&expired, &worktree, &mutation);
    let mut stopped = current.clone();
    stopped.requested_stop_reason = Some(AgentStopReason::Owner);
    assert_conflict(&stopped, &worktree, &mutation);
    let mut mismatched = worktree.clone();
    mismatched.status = WorktreeStatus::Removing;
    assert_conflict(&current, &mismatched, &mutation);
    assert_eq!(
        validate_agent_work_mutation(
            &current,
            &worktree,
            current.conversation.last_seq + 1,
            &mutation
        ),
        Err(SandboxStoreError::SequenceConflict)
    );
}

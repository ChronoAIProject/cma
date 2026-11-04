use super::*;
use layer2_core::sandbox::store::{
    plan_agent_cleanup_capacity_retry, validate_agent_cleanup_capacity_log,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use std::time::{Duration, UNIX_EPOCH};

fn fixture(step: AgentWorkStep) -> Agent {
    let mut agent = conversation_agent_for_rules("agt_cleanup_retry_capacity");
    agent = ready_publication_claim(agent, "retained-conversation");
    agent.status = match step {
        AgentWorkStep::EnsureProcessClosed
        | AgentWorkStep::RemoveHome
        | AgentWorkStep::RemoveWorktree
        | AgentWorkStep::PublishDeleted => AgentStatus::Stopped,
        _ => AgentStatus::Ready,
    };
    agent.requested_stop_reason = agent.status.is_live().then_some(AgentStopReason::Owner);
    agent.work.step = step;
    agent.work.lease = None;
    agent
        .work
        .claim("worker".into(), NOW, MINUTE.into())
        .unwrap();
    agent.conversation.last_seq = 10_000;
    agent
}

fn plan(agent: &Agent, last: Option<&AgentLogEntry>) -> AgentWorkMutation {
    plan_agent_cleanup_capacity_retry(
        agent,
        last,
        "worker",
        agent.work.lease.as_ref().unwrap().fence,
        NOW,
        MINUTE,
    )
    .unwrap()
}

fn marker(publication: &AgentWorkMutation) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: publication.agent.id.clone(),
        seq: publication.agent.conversation.last_seq,
        appended_at: publication.appended_at.clone(),
        kind: publication.agent_logs[0].clone(),
    }
}

#[test]
fn cleanup_capacity_retry_is_bounded_without_losing_attempts_or_effect_evidence() {
    for step in [
        AgentWorkStep::FenceInteraction,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ] {
        let mut current = fixture(step);
        let original = current.clone();
        let mut last = None;
        let epoch = seconds_from_rfc3339(NOW).unwrap();
        for attempt in 0..130 {
            let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(epoch + attempt * 60));
            let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(epoch + (attempt + 1) * 60));
            if attempt != 0 {
                current
                    .work
                    .claim("worker".into(), &at, due.clone())
                    .unwrap();
                current.revision += 1;
            }
            let fence = current.work.lease.as_ref().unwrap().fence;
            let mutation = plan_agent_cleanup_capacity_retry(
                &current,
                last.as_ref(),
                "worker",
                fence,
                &at,
                &due,
            )
            .unwrap();
            validate_agent_cleanup_capacity_log(&current, &mutation, last.as_ref()).unwrap();
            assert_eq!(
                publication_capacity(&current, &mutation),
                if current.status.is_live() {
                    AppendCapacity::StopOrFailure
                } else {
                    AppendCapacity::Delete
                }
            );
            assert_eq!(mutation.agent_logs.len(), usize::from(attempt == 0));
            assert_eq!(mutation.agent.work.attempts, attempt as u32 + 1);
            assert_eq!(mutation.agent.work.ready_at, due);
            assert!(mutation.agent.work.lease.is_none());
            assert_eq!(mutation.agent.work.receipts, original.work.receipts);
            assert_eq!(mutation.agent.work.cleanup_debt, original.work.cleanup_debt);
            assert_eq!(mutation.agent.status, original.status);
            if attempt == 0 {
                last = Some(marker(&mutation));
            }
            current = mutation.agent;
        }
        assert_eq!(
            current.conversation.last_seq,
            original.conversation.last_seq + 1
        );
    }
}

#[test]
fn cleanup_capacity_retry_requires_exact_retained_marker_and_refuses_duplicates() {
    let current = fixture(AgentWorkStep::CloseProcess);
    let first = plan(&current, None);
    let last = marker(&first);
    let mut current = first.agent;
    current
        .work
        .claim("worker".into(), MINUTE, TWO_MINUTES.into())
        .unwrap();
    current.revision += 1;
    let fence = current.work.lease.as_ref().unwrap().fence;
    let retry = plan_agent_cleanup_capacity_retry(
        &current,
        Some(&last),
        "worker",
        fence,
        MINUTE,
        TWO_MINUTES,
    )
    .unwrap();
    assert!(retry.agent_logs.is_empty());
    for variant in 0..5 {
        let mut invalid = last.clone();
        match variant {
            0 => invalid.agent_id = AgentId("agt_different".into()),
            1 => invalid.seq -= 1,
            2 => {
                if let AgentLogKind::WorkStepFailed { step, .. } = &mut invalid.kind {
                    *step = AgentWorkStep::EnsureProcessClosed;
                }
            }
            3 => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut invalid.kind {
                    failure.detail.push('!');
                }
            }
            _ => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut invalid.kind {
                    failure.class = "agent_runtime_unavailable".into();
                }
            }
        }
        assert_eq!(
            validate_agent_cleanup_capacity_log(&current, &retry, Some(&invalid)),
            Err(SandboxStoreError::Conflict)
        );
    }
    assert_eq!(
        validate_agent_cleanup_capacity_log(&current, &retry, None),
        Err(SandboxStoreError::Conflict)
    );
    let duplicate =
        plan_agent_cleanup_capacity_retry(&current, None, "worker", fence, MINUTE, TWO_MINUTES)
            .unwrap();
    assert_eq!(
        validate_agent_cleanup_capacity_log(&current, &duplicate, Some(&last)),
        Err(SandboxStoreError::Conflict)
    );
}

#[test]
fn cleanup_capacity_retry_rejects_expired_or_changed_authority() {
    let current = fixture(AgentWorkStep::CloseProcess);
    let first = plan(&current, None);
    let fence = current.work.lease.as_ref().unwrap().fence;
    for (holder, fence, at, due) in [
        ("other", fence, NOW, MINUTE),
        ("worker", fence + 1, NOW, MINUTE),
        ("worker", fence, MINUTE, TWO_MINUTES),
        ("worker", fence, NOW, NOW),
    ] {
        assert!(matches!(
            plan_agent_cleanup_capacity_retry(&current, None, holder, fence, at, due,),
            Err(SandboxStoreError::Conflict)
        ));
    }
    for variant in 0..6 {
        let mut invalid = first.clone();
        match variant {
            0 => invalid.agent.work.attempts += 1,
            1 => invalid.agent.conversation.conversation_id = Some("other-conversation".into()),
            2 => invalid.agent.requested_stop_reason = None,
            3 => invalid.agent.owner = SubjectId("other-owner".into()),
            4 => invalid.agent.revision += 1,
            _ => invalid.agent.work.next_fence += 1,
        }
        assert_eq!(
            validate_agent_cleanup_capacity_log(&current, &invalid, None),
            Err(SandboxStoreError::Conflict),
            "variant {variant}"
        );
    }
    for step in [
        AgentWorkStep::PublishConnection,
        AgentWorkStep::OpenConversation,
    ] {
        assert!(matches!(
            plan_agent_cleanup_capacity_retry(&fixture(step), None, "worker", fence, NOW, MINUTE,),
            Err(SandboxStoreError::Conflict)
        ));
    }
}

use super::*;
use crate::sandbox::store::is_prepared_agent_replay_pair;

#[test]
fn prepared_pair_intent_and_completion_preserve_the_exact_effects() {
    let (current, worktree) = prepared();
    let intent = pair_intent(&current);
    assert_valid(&current, &worktree, &intent);
    assert!(is_prepared_agent_replay_pair(&current, &intent));
    let durable: Agent =
        serde_json::from_value(serde_json::to_value(&intent.agent).unwrap()).unwrap();
    assert_eq!(durable, intent.agent);
    let completion = pair_completion(&durable);
    assert_valid(&durable, &worktree, &completion);
    assert!(is_prepared_agent_replay_pair(&durable, &completion));
    assert_eq!(completion.agent.work.step, AgentWorkStep::OpenConversation);
    assert_eq!(
        completion.agent.conversation.last_seq,
        current.conversation.last_seq + 2
    );
    assert_eq!(completion.agent.work.next_fence, current.work.next_fence);
    assert!(completion.agent.work.lease.is_none());
    assert!(completion.agent.conversation.conversation_id.is_none());
}

#[test]
fn paired_intent_requires_prepared_lifecycle_and_resolved_empty_skills() {
    for change in 0..6 {
        let (mut current, worktree) = prepared();
        match change {
            0 => current.work.warm_conversation_id = None,
            1 => current.work.warm_conversation_id = Some(" ".into()),
            2 => current.work.resolved_skills = None,
            3 => current.origin = AgentOrigin::Manual,
            4 => current
                .profile
                .content
                .skills
                .push(crate::profiles::SkillRef {
                    source: crate::profiles::SkillSource::Ornn,
                    id: crate::SkillId::new("research"),
                    version: None,
                }),
            _ => current.conversation.conversation_id = Some("already-open".into()),
        }
        let mutation = pair_intent(&current);
        assert!(!is_prepared_agent_replay_pair(&current, &mutation));
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_intent_accepts_only_two_ordered_unique_valid_debts() {
    let (current, worktree) = prepared();
    for change in 0..6 {
        let mut mutation = pair_intent(&current);
        let debts = &mut mutation.agent.work.cleanup_debt;
        match change {
            0 => debts.swap(0, 1),
            1 => debts[1] = debts[0].clone(),
            2 => debts.push(debt(&current, AgentWorkStep::OpenConversation)),
            3 => debts[1].operation_id = "different-home".into(),
            4 => debts[0].request_sha256 = "A".repeat(64),
            _ => debts[0].operation_id.clear(),
        }
        assert!(!is_prepared_agent_replay_pair(&current, &mutation));
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_intent_cannot_replace_existing_debt_or_receipt() {
    for receipt_exists in [false, true] {
        let (mut current, worktree) = prepared();
        if receipt_exists {
            current
                .work
                .receipts
                .push(receipt(&current, AgentWorkStep::AddWorktree));
        } else {
            current
                .work
                .cleanup_debt
                .push(debt(&current, AgentWorkStep::AddWorktree));
        }
        let mut mutation = pair_intent(&current);
        mutation.agent.work.receipts.clear();
        assert!(!is_prepared_agent_replay_pair(&current, &mutation));
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_intent_keeps_lease_fence_selection_and_due_state() {
    let (current, worktree) = prepared();
    for change in 0..8 {
        let mut mutation = pair_intent(&current);
        match change {
            0 => mutation.agent.work.lease = None,
            1 => mutation.agent.work.lease.as_mut().unwrap().fence += 1,
            2 => mutation.agent.work.next_fence += 1,
            3 => mutation.agent.work.attempts += 1,
            4 => mutation.agent.work.ready_at = RETRY_AT.into(),
            5 => mutation.agent.work.warm_conversation_id = Some("different".into()),
            6 => mutation.agent.work.resolved_skills = None,
            _ => mutation.agent.work.step = AgentWorkStep::MaterializeHome,
        }
        assert!(!is_prepared_agent_replay_pair(&current, &mutation));
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_completion_requires_both_original_receipts_in_order_at_publication() {
    let (current, worktree) = prepared();
    let current = pair_intent(&current).agent;
    for change in 0..8 {
        let mut mutation = pair_completion(&current);
        match change {
            0 => {
                mutation.agent.work.receipts.pop();
            }
            1 => mutation.agent.work.receipts.swap(0, 1),
            2 => mutation.agent.work.receipts[0].request_sha256 = "b".repeat(64),
            3 => mutation.agent.work.receipts[0].operation_id = "different-worktree".into(),
            4 => mutation.agent.work.receipts[1].completed_at = NOW.into(),
            5 => mutation
                .agent
                .work
                .receipts
                .push(receipt(&current, AgentWorkStep::OpenConversation)),
            6 => mutation.agent.work.cleanup_debt = current.work.cleanup_debt.clone(),
            _ => mutation.agent.work.step = AgentWorkStep::PublishReady,
        }
        assert!(!is_prepared_agent_replay_pair(&current, &mutation));
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_logs_are_exact_and_cannot_publish_worktree_or_conversation_readiness() {
    let (current, worktree) = prepared();
    let current = pair_intent(&current).agent;
    for change in 0..8 {
        let mut mutation = pair_completion(&current);
        match change {
            0 => {
                mutation.agent_logs.pop();
            }
            1 => mutation.agent_logs.swap(0, 1),
            2 => mutation
                .agent_logs
                .push(completed(AgentWorkStep::OpenConversation)),
            3 => mutation.agent_logs[1] = failed(AgentWorkStep::MaterializeHome, "lost_reply"),
            4 => mutation.worktree = Some(versioned(&worktree, worktree.clone())),
            5 => {
                mutation.sandbox_event = Some(SandboxEventKind::AgentDeleted {
                    agent_id: current.id.clone(),
                })
            }
            6 => mutation.agent.conversation.conversation_id = Some("forged-ready".into()),
            _ => mutation.agent.ready_at = Some(AT.into()),
        }
        mutation.agent.conversation.last_seq =
            current.conversation.last_seq + mutation.agent_logs.len() as u64;
        assert_conflict(&current, &worktree, &mutation);
    }
}

#[test]
fn paired_publications_do_not_bypass_stop_delete_revision_or_worker_fences() {
    let (initial, worktree) = prepared();
    for completing in [false, true] {
        let original = if completing {
            pair_intent(&initial).agent
        } else {
            initial.clone()
        };
        for change in 0..6 {
            let mut current = original.clone();
            let mut mutation = if completing {
                pair_completion(&current)
            } else {
                pair_intent(&current)
            };
            match change {
                0 => current.requested_stop_reason = Some(AgentStopReason::Owner),
                1 => current.status = AgentStatus::Deleted,
                2 => current.revision += 1,
                3 => current.work.lease.as_mut().unwrap().holder = "successor".into(),
                4 => current.work.lease.as_mut().unwrap().fence += 1,
                _ => mutation.agent.profile.digest = "e".repeat(64),
            }
            assert_conflict(&current, &worktree, &mutation);
        }
    }
}

#[test]
fn paired_debt_survives_retry_restart_and_normal_single_step_recovery() {
    let (initial, worktree) = prepared();
    let current = pair_intent(&initial).agent;
    let retry = retry_publication(&current);
    assert_valid(&current, &worktree, &retry);
    assert!(!is_prepared_agent_replay_pair(&current, &retry));
    assert_eq!(retry.agent.work.cleanup_debt, current.work.cleanup_debt);
    let mut restarted: Agent =
        serde_json::from_value(serde_json::to_value(&retry.agent).unwrap()).unwrap();
    restarted
        .work
        .claim("worker".into(), RETRY_AT, LEASE_END.into())
        .unwrap();
    restarted.revision += 1;
    restarted.updated_at = RETRY_AT.into();
    let mut add = completed_advance(&restarted, AgentWorkStep::MaterializeHome);
    complete_one(&mut add, &restarted, AgentWorkStep::AddWorktree);
    at_time(&mut add, "2026-09-05T00:00:30Z");
    assert_valid(&restarted, &worktree, &add);
    assert!(!is_prepared_agent_replay_pair(&restarted, &add));
    assert_eq!(
        add.agent.work.cleanup_debt,
        vec![debt(&initial, AgentWorkStep::MaterializeHome)]
    );
    let mut home = add.agent;
    home.work
        .claim("worker".into(), "2026-09-05T00:00:30Z", LEASE_END.into())
        .unwrap();
    home.revision += 1;
    let mut completed_home = completed_advance(&home, AgentWorkStep::OpenConversation);
    complete_one(&mut completed_home, &home, AgentWorkStep::MaterializeHome);
    at_time(&mut completed_home, "2026-09-05T00:00:40Z");
    assert_valid(&home, &worktree, &completed_home);
    assert_eq!(completed_home.agent.work.receipts.len(), 2);
    assert!(completed_home.agent.work.cleanup_debt.is_empty());
}

fn prepared() -> (Agent, Worktree) {
    let (mut agent, worktree) = claimed(
        AgentWorkStep::AddWorktree,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    agent.origin = AgentOrigin::Lifecycle {
        operation_id: "prepared-origin".into(),
    };
    agent.work.warm_conversation_id = Some("prepared-conversation".into());
    agent.work.resolved_skills = Some(vec![]);
    (agent, worktree)
}

fn pair_intent(current: &Agent) -> AgentWorkMutation {
    let mut next = current.clone();
    next.work.cleanup_debt = vec![
        debt(current, AgentWorkStep::AddWorktree),
        debt(current, AgentWorkStep::MaterializeHome),
    ];
    publication(current, next, vec![], None, None)
}

fn pair_completion(current: &Agent) -> AgentWorkMutation {
    let mut next = current.clone();
    next.work
        .advance(
            "worker",
            fence(current),
            AgentWorkStep::OpenConversation,
            AT.into(),
        )
        .unwrap();
    next.work.receipts = current
        .work
        .cleanup_debt
        .iter()
        .map(|debt| AgentWorkReceipt {
            step: debt.step,
            operation_id: debt.operation_id.clone(),
            request_sha256: debt.request_sha256.clone(),
            completed_at: AT.into(),
        })
        .collect();
    next.work.cleanup_debt.clear();
    publication(
        current,
        next,
        vec![
            completed(AgentWorkStep::AddWorktree),
            completed(AgentWorkStep::MaterializeHome),
        ],
        None,
        None,
    )
}

fn complete_one(mutation: &mut AgentWorkMutation, current: &Agent, step: AgentWorkStep) {
    mutation
        .agent
        .work
        .cleanup_debt
        .retain(|debt| debt.step != step);
    let mut completed = receipt(current, step);
    completed.completed_at = AT.into();
    mutation.agent.work.receipts.push(completed);
}

fn at_time(mutation: &mut AgentWorkMutation, time: &str) {
    mutation.appended_at = time.into();
    mutation.agent.updated_at = time.into();
    mutation.agent.work.ready_at = time.into();
    mutation
        .agent
        .work
        .receipts
        .last_mut()
        .unwrap()
        .completed_at = time.into();
}

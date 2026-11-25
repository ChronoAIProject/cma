use crate::ids::{AgentId, AgentProfileId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use crate::profiles::{AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort};
use crate::sandbox::{
    Agent, AgentCleanupDebt, AgentLogKind, AgentOrigin, AgentStatus, AgentStopReason,
    AgentWorkReceipt, AgentWorkState, AgentWorkStep, ConversationState, FirstMessageRecord,
    SandboxEventKind, TypedFailure, Worktree, WorktreeStatus,
};

use super::{
    validate_agent_work_mutation, AgentWorkMutation, SandboxStoreError, VersionedWorktree,
    MAX_AGENT_CLEANUP_DEBT, MAX_AGENT_WORK_RECEIPTS,
};

const NOW: &str = "2026-09-05T00:00:00Z";
const AT: &str = "2026-09-05T00:00:10Z";
const RETRY_AT: &str = "2026-09-05T00:00:20Z";
const LEASE_END: &str = "2026-09-05T00:01:00Z";

mod capacity_tests;
mod prepared_open_ready_tests;
mod prepared_pair_tests;
mod process_cleanup_tests;
mod retired_runtime_tests;
mod sign_in_tests;
mod startup_failure_tests;
mod startup_hold_tests;

#[test]
fn warm_home_intent_preserves_the_prepared_skill_selection_and_session() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::MaterializeHome,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::AddWorktree));
    current.work.warm_conversation_id = Some("prepared-session".into());
    current.work.resolved_skills = Some(vec![]);
    let mut next = current.clone();
    next.work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::MaterializeHome));
    let intent = publication(&current, next, vec![], None, None);
    assert_valid(&current, &worktree, &intent);
    let mut erased = intent.clone();
    erased.agent.work.resolved_skills = None;
    assert_conflict(&current, &worktree, &erased);
    for conversation in [None, Some("different-session".into())] {
        let mut changed = intent.clone();
        changed.agent.work.warm_conversation_id = conversation;
        assert_conflict(&current, &worktree, &changed);
    }
    let mut unwarmed = current.clone();
    unwarmed.work.warm_conversation_id = None;
    let mut forged = intent;
    forged.agent.work.warm_conversation_id = None;
    assert_conflict(&unwarmed, &worktree, &forged);
}

#[test]
fn skill_material_is_committed_once_with_the_first_home_intent() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::MaterializeHome,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::AddWorktree));
    current.profile.content.skills = vec![crate::profiles::SkillRef {
        source: crate::profiles::SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        version: None,
    }];
    let material = vec![crate::sandbox::ResolvedAgentSkill {
        source: crate::profiles::SkillSource::Ornn,
        id: crate::SkillId::new("research"),
        name: Some("research".into()),
        version: "2.10".into(),
        archive_sha256: "a".repeat(64),
    }];
    let mut next = current.clone();
    next.work.resolved_skills = Some(material);
    next.work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::MaterializeHome));
    let intent = publication(&current, next, vec![], None, None);
    assert_valid(&current, &worktree, &intent);
    let mut missing_material = intent.clone();
    missing_material.agent.work.resolved_skills = None;
    assert_conflict(&current, &worktree, &missing_material);
    let mut missing_debt = intent.clone();
    missing_debt.agent.work.cleanup_debt.clear();
    assert_conflict(&current, &worktree, &missing_debt);
    for mutate in [0, 1, 2, 3] {
        let mut invalid = intent.clone();
        let material = invalid.agent.work.resolved_skills.as_mut().unwrap();
        match mutate {
            0 => material[0].id = crate::SkillId::new("different"),
            1 => material[0].version.clear(),
            2 => material[0].archive_sha256 = "A".repeat(64),
            _ => material.push(material[0].clone()),
        }
        assert_conflict(&current, &worktree, &invalid);
    }
    let selected = intent.agent;
    let retry = retry_publication(&selected);
    assert_valid(&selected, &worktree, &retry);
    for material in [None, Some(vec![])] {
        let mut replacement = retry.clone();
        replacement.agent.work.resolved_skills = material;
        assert_conflict(&selected, &worktree, &replacement);
    }
    let mut advance = completed_advance(&selected, AgentWorkStep::OpenConversation);
    advance.agent.work.cleanup_debt.clear();
    let mut completed = receipt(&selected, AgentWorkStep::MaterializeHome);
    completed.completed_at = AT.into();
    advance.agent.work.receipts.push(completed);
    assert_valid(&selected, &worktree, &advance);
    let mut replacement = advance.clone();
    replacement.agent.work.resolved_skills.as_mut().unwrap()[0].archive_sha256 = "b".repeat(64);
    assert_conflict(&selected, &worktree, &replacement);
    let mut replacement = advance;
    replacement.agent.work.resolved_skills.as_mut().unwrap()[0].name = Some("renamed".into());
    assert_conflict(&selected, &worktree, &replacement);
    assert_eq!(selected.profile, current.profile);
}

#[test]
fn legacy_skill_material_without_name_round_trips_and_new_names_are_validated() {
    let legacy = serde_json::json!({
        "source": "ornn",
        "id": "research",
        "version": "1.0",
        "archive_sha256": "a".repeat(64),
    });
    let mut material: crate::sandbox::ResolvedAgentSkill =
        serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(material.name, None);
    assert_eq!(serde_json::to_value(&material).unwrap(), legacy);
    assert!(material.validate().is_ok());
    for name in ["", " spaced", "control\n"] {
        material.name = Some(name.into());
        assert!(material.validate().is_err());
    }
}

#[test]
fn resolved_empty_is_distinct_from_unresolved_and_survives_roundtrip() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::MaterializeHome,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::AddWorktree));
    let mut next = current.clone();
    next.work.resolved_skills = Some(vec![]);
    next.work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::MaterializeHome));
    let intent = publication(&current, next, vec![], None, None);
    assert_valid(&current, &worktree, &intent);
    let unresolved = serde_json::to_value(&current).unwrap();
    assert!(unresolved["work"].get("resolved_skills").is_none());
    let resolved = serde_json::to_value(&intent.agent).unwrap();
    assert_eq!(resolved["work"]["resolved_skills"], serde_json::json!([]));
    assert_eq!(
        serde_json::from_value::<Agent>(resolved).unwrap(),
        intent.agent
    );
    let mut legacy = intent.agent.clone();
    legacy.work.resolved_skills = None;
    assert_conflict(
        &legacy,
        &worktree,
        &completed_advance(&legacy, AgentWorkStep::OpenConversation),
    );
}

#[test]
fn intent_retry_and_startup_advance_have_exact_work_deltas() {
    let (current, worktree) = claimed(
        AgentWorkStep::AddWorktree,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );

    let mut intent_agent = current.clone();
    intent_agent
        .work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::AddWorktree));
    let intent = publication(&current, intent_agent, vec![], None, None);
    assert_valid(&current, &worktree, &intent);

    let mut invalid_intent = intent.clone();
    invalid_intent.agent.work.lease = None;
    assert_conflict(&current, &worktree, &invalid_intent);
    let mut invalid_intent = intent.clone();
    invalid_intent
        .agent_logs
        .push(completed(AgentWorkStep::AddWorktree));
    invalid_intent.agent.conversation.last_seq += 1;
    assert_conflict(&current, &worktree, &invalid_intent);

    let intent_current = intent.agent.clone();
    let mut advanced = intent_current.clone();
    let mut evidence = receipt(&advanced, AgentWorkStep::AddWorktree);
    evidence.completed_at = AT.into();
    advanced.work.cleanup_debt.clear();
    advanced.work.receipts.push(evidence);
    advanced
        .work
        .advance(
            "worker",
            fence(&intent_current),
            AgentWorkStep::MaterializeHome,
            AT.into(),
        )
        .unwrap();
    let advanced = publication(
        &intent_current,
        advanced,
        vec![completed(AgentWorkStep::AddWorktree)],
        None,
        None,
    );
    assert_valid(&intent_current, &worktree, &advanced);

    let mut changed_grants = advanced.clone();
    changed_grants.agent.capabilities = crate::RuntimeCapabilities::new([
        crate::RuntimeCapability::Conversation,
        crate::RuntimeCapability::Resources,
    ]);
    assert_conflict(&intent_current, &worktree, &changed_grants);

    let mut mismatched = advanced.clone();
    mismatched
        .agent
        .work
        .receipts
        .last_mut()
        .unwrap()
        .request_sha256 = "b".repeat(64);
    assert_conflict(&intent_current, &worktree, &mismatched);
    let mut skipped = advanced.clone();
    skipped.agent.work.step = AgentWorkStep::OpenConversation;
    assert_conflict(&intent_current, &worktree, &skipped);

    let mut retry_agent = current.clone();
    retry_agent
        .work
        .retry("worker", fence(&current), RETRY_AT.into())
        .unwrap();
    let retry = publication(
        &current,
        retry_agent,
        vec![failed(AgentWorkStep::AddWorktree, "runtime_unavailable")],
        None,
        None,
    );
    assert_valid(&current, &worktree, &retry);

    let mut invalid_retry = retry.clone();
    invalid_retry.agent.work.attempts += 1;
    assert_conflict(&current, &worktree, &invalid_retry);
    let mut invalid_retry = retry.clone();
    invalid_retry.agent.work.ready_at = AT.into();
    assert_conflict(&current, &worktree, &invalid_retry);
    let mut invalid_retry = retry.clone();
    invalid_retry.agent_logs = vec![completed(AgentWorkStep::AddWorktree)];
    assert_conflict(&current, &worktree, &invalid_retry);
}

#[test]
fn only_effect_free_parent_wait_can_remain_due_at_publication() {
    let (mut current, mut worktree) = claimed(
        AgentWorkStep::WaitForWorkspace,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current.origin = AgentOrigin::Lifecycle {
        operation_id: "parent-wait".into(),
    };
    worktree.base_commit.clear();
    let mut waiting = current.clone();
    waiting
        .work
        .retry("worker", fence(&current), AT.into())
        .unwrap();
    let due = publication(&current, waiting.clone(), vec![], None, None);
    assert_valid(&current, &worktree, &due);

    let mut earlier = due.clone();
    earlier.agent.work.ready_at = NOW.into();
    assert_conflict(&current, &worktree, &earlier);

    let mut failure = publication(
        &current,
        waiting,
        vec![failed(AgentWorkStep::WaitForWorkspace, "store_unavailable")],
        None,
        None,
    );
    assert_conflict(&current, &worktree, &failure);
    failure.agent.work.ready_at = RETRY_AT.into();
    assert_valid(&current, &worktree, &failure);

    let mut effected = current.clone();
    effected
        .work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::AddWorktree));
    let mut forged = due;
    forged.agent.work.cleanup_debt = effected.work.cleanup_debt.clone();
    assert_conflict(&effected, &worktree, &forged);
}

#[test]
fn ready_publication_couples_identity_status_worktree_logs_and_event() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::PublishReady,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    add_startup_receipts(&mut current);
    current.conversation.conversation_id = Some("conversation-1".into());
    current.conversation.runtime_generation = Some(1);

    let mut next = current.clone();
    next.work
        .advance(
            "worker",
            fence(&current),
            AgentWorkStep::PublishConnection,
            AT.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, AT.into(), None)
        .unwrap();
    let mut ready_worktree = worktree.clone();
    ready_worktree.transition(WorktreeStatus::Ready).unwrap();
    let logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-1".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    let event = SandboxEventKind::AgentStatusChanged {
        agent_id: current.id.clone(),
        from: AgentStatus::Starting,
        to: AgentStatus::Ready,
    };
    let ready = publication(
        &current,
        next,
        logs,
        Some(event),
        Some(versioned(&worktree, ready_worktree)),
    );
    assert_valid(&current, &worktree, &ready);

    let mut invalid = ready.clone();
    invalid.sandbox_event = None;
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = ready.clone();
    invalid.agent.ready_at = Some(RETRY_AT.into());
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = ready.clone();
    invalid.agent_logs.swap(0, 1);
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = ready.clone();
    invalid.worktree.as_mut().unwrap().worktree.status = WorktreeStatus::Failed;
    assert_conflict(&current, &worktree, &invalid);
}

#[test]
fn startup_failure_preserves_debt_and_requires_matching_failure_evidence() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::MaterializeHome,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::AddWorktree));
    current
        .work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::MaterializeHome));
    let failure = typed("home_rejected");
    let mut next = current.clone();
    next.work
        .advance("worker", fence(&current), AgentWorkStep::None, AT.into())
        .unwrap();
    next.fail(failure.clone(), AT.into()).unwrap();
    let mut failed_worktree = worktree.clone();
    failed_worktree.transition(WorktreeStatus::Failed).unwrap();
    let failed = publication(
        &current,
        next,
        vec![AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::MaterializeHome,
            failure: failure.clone(),
        }],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: current.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Failed,
        }),
        Some(versioned(&worktree, failed_worktree)),
    );
    assert_valid(&current, &worktree, &failed);

    let mut invalid = failed.clone();
    invalid.agent.work.cleanup_debt.clear();
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = failed.clone();
    invalid.agent.failure = Some(typed("another_failure"));
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = failed.clone();
    invalid
        .agent
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::MaterializeHome));
    assert_conflict(&current, &worktree, &invalid);
}

#[test]
fn owner_stop_settlement_interrupts_only_the_captured_turn() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::PublishStopped,
        AgentStatus::Working,
        WorktreeStatus::Ready,
    );
    current.requested_stop_reason = Some(AgentStopReason::Owner);
    add_startup_receipts(&mut current);
    current.conversation.active_turn = Some(2);
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::CloseProcess));

    let mut stopped = current.clone();
    let interrupted = stopped.settle_owner_stop_turn().unwrap();
    assert_eq!(
        interrupted,
        AgentLogKind::TurnInterrupted { turn_sequence: 2 }
    );
    assert_eq!(stopped.settle_owner_stop_turn(), None);
    stopped
        .work
        .advance("worker", fence(&current), AgentWorkStep::None, AT.into())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            AT.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    let proposal = publication(
        &current,
        stopped,
        vec![
            interrupted,
            AgentLogKind::Stopped {
                reason: "owner".into(),
            },
        ],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: current.id.clone(),
            from: AgentStatus::Working,
            to: AgentStatus::Stopped,
        }),
        None,
    );
    assert_valid(&current, &worktree, &proposal);
    let mut wrong_turn = proposal.clone();
    wrong_turn.agent_logs[0] = AgentLogKind::TurnInterrupted { turn_sequence: 3 };
    assert_conflict(&current, &worktree, &wrong_turn);
    let mut stale = current.clone();
    stale.conversation.active_turn = None;
    assert_conflict(&stale, &worktree, &proposal);
}

#[test]
fn stop_and_delete_publications_consume_only_their_exact_terminal_state() {
    let (mut stopping, ready_worktree) = claimed(
        AgentWorkStep::PublishStopped,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut stopping);
    stopping.requested_stop_reason = Some(AgentStopReason::Owner);
    stopping
        .work
        .receipts
        .push(receipt(&stopping, AgentWorkStep::CloseProcess));
    let mut stopped = stopping.clone();
    stopped
        .work
        .advance("worker", fence(&stopping), AgentWorkStep::None, AT.into())
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            AT.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    let stop = publication(
        &stopping,
        stopped,
        vec![AgentLogKind::Stopped {
            reason: "owner".into(),
        }],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: stopping.id.clone(),
            from: AgentStatus::Ready,
            to: AgentStatus::Stopped,
        }),
        None,
    );
    assert_valid(&stopping, &ready_worktree, &stop);

    let mut invalid = stop.clone();
    invalid.agent.stop_reason = Some(AgentStopReason::Operator);
    assert_conflict(&stopping, &ready_worktree, &invalid);
    let mut invalid = stop.clone();
    invalid.agent.work.step = AgentWorkStep::PublishDeleted;
    assert_conflict(&stopping, &ready_worktree, &invalid);

    let (mut deleting, removed_worktree) = claimed(
        AgentWorkStep::PublishDeleted,
        AgentStatus::Stopped,
        WorktreeStatus::Removed,
    );
    add_cleanup_receipts(&mut deleting);
    let mut deleted = deleting.clone();
    deleted
        .work
        .advance("worker", fence(&deleting), AgentWorkStep::None, AT.into())
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, AT.into(), None)
        .unwrap();
    let delete = publication(
        &deleting,
        deleted,
        vec![AgentLogKind::Deleted],
        Some(SandboxEventKind::AgentDeleted {
            agent_id: deleting.id.clone(),
        }),
        None,
    );
    assert_valid(&deleting, &removed_worktree, &delete);
    // Deleted is durable proof that the exact owned worktree finished removal.
    // Runtime-update admission may rely on this when retained turns are history.
    for variant in 0..6 {
        let mut invalid_worktree = removed_worktree.clone();
        match variant {
            0 => invalid_worktree.status = WorktreeStatus::Removing,
            1 => invalid_worktree.id = crate::ids::WorktreeId("foreign".into()),
            2 => invalid_worktree.workspace_id = crate::ids::WorkspaceId("foreign".into()),
            3 => invalid_worktree.sandbox_id = crate::ids::SandboxId("foreign".into()),
            4 => invalid_worktree.owner = crate::ids::SubjectId("foreign".into()),
            _ => invalid_worktree.agent_id = Some(crate::ids::AgentId("foreign".into())),
        }
        assert_conflict(&deleting, &invalid_worktree, &delete);
    }

    let mut invalid = delete.clone();
    invalid
        .agent
        .work
        .cleanup_debt
        .push(debt(&deleting, AgentWorkStep::AddWorktree));
    assert_conflict(&deleting, &removed_worktree, &invalid);
    let mut invalid = delete.clone();
    invalid.sandbox_event = Some(SandboxEventKind::AgentStatusChanged {
        agent_id: deleting.id.clone(),
        from: AgentStatus::Stopped,
        to: AgentStatus::Deleted,
    });
    assert_conflict(&deleting, &removed_worktree, &invalid);
}

#[test]
fn cleanup_advance_settles_only_the_required_compensation() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::RemoveWorktree,
        AgentStatus::Failed,
        WorktreeStatus::Removing,
    );
    current
        .work
        .receipts
        .push(receipt(&current, AgentWorkStep::AddWorktree));
    current.work.cleanup_debt.clear();
    let mut next = current.clone();
    let mut removal = receipt(&current, AgentWorkStep::RemoveWorktree);
    removal.completed_at = AT.into();
    next.work.receipts.push(removal);
    next.work
        .advance(
            "worker",
            fence(&current),
            AgentWorkStep::PublishDeleted,
            AT.into(),
        )
        .unwrap();
    let mut removed = worktree.clone();
    removed.transition(WorktreeStatus::Removed).unwrap();
    let mutation = publication(
        &current,
        next,
        vec![completed(AgentWorkStep::RemoveWorktree)],
        Some(SandboxEventKind::WorktreeRemoved {
            workspace_id: worktree.workspace_id.clone(),
            worktree_id: worktree.id.clone(),
        }),
        Some(versioned(&worktree, removed)),
    );
    assert_valid(&current, &worktree, &mutation);

    let mut invalid = mutation.clone();
    invalid.agent.work.receipts.pop();
    assert_conflict(&current, &worktree, &invalid);
    let mut invalid = mutation.clone();
    invalid.sandbox_event = None;
    assert_conflict(&current, &worktree, &invalid);
}

#[test]
fn elapsed_cleanup_delay_allows_fenced_agent_shutdown_to_continue() {
    for deadline in [NOW, AT] {
        let (mut current, worktree) = claimed(
            AgentWorkStep::FenceInteraction,
            AgentStatus::Ready,
            WorktreeStatus::Ready,
        );
        current.work.cleanup_not_before = Some(deadline.into());
        let mut next = current.clone();
        next.work
            .advance(
                "worker",
                fence(&current),
                AgentWorkStep::CloseProcess,
                AT.into(),
            )
            .unwrap();
        assert_eq!(next.work.ready_at, AT);
        assert!(next.work.cleanup_not_before.is_none());
        let mutation = publication(
            &current,
            next,
            vec![completed(AgentWorkStep::FenceInteraction)],
            None,
            None,
        );
        assert_valid(&current, &worktree, &mutation);
        let mut premature = mutation.clone();
        premature.agent.work.ready_at = NOW.into();
        assert_conflict(&current, &worktree, &premature);
    }
}

#[test]
fn immutable_fields_histories_and_delayed_readiness_are_enforced() {
    let (current, worktree) = claimed(
        AgentWorkStep::FenceInteraction,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    let mut delayed = current.clone();
    delayed.work.cleanup_not_before = Some(LEASE_END.into());
    let mut next = delayed.clone();
    next.work
        .advance(
            "worker",
            fence(&delayed),
            AgentWorkStep::CloseProcess,
            AT.into(),
        )
        .unwrap();
    let mutation = publication(
        &delayed,
        next,
        vec![completed(AgentWorkStep::FenceInteraction)],
        None,
        None,
    );
    assert_valid(&delayed, &worktree, &mutation);
    let mut invalid = mutation.clone();
    invalid.agent.work.ready_at = AT.into();
    assert_conflict(&delayed, &worktree, &invalid);
    let mut invalid = mutation.clone();
    invalid.agent.title = Some("rewritten".into());
    assert_conflict(&delayed, &worktree, &invalid);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.runtime_generation = Some(2);
    assert_conflict(&delayed, &worktree, &invalid);
    let mut invalid = mutation.clone();
    invalid.agent.work.next_fence += 1;
    assert_conflict(&delayed, &worktree, &invalid);

    let (mut overflowing, overflow_worktree) = claimed(
        AgentWorkStep::PublishConnection,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    overflowing.work.receipts = (0..=MAX_AGENT_WORK_RECEIPTS)
        .map(|_| receipt(&overflowing, AgentWorkStep::AddWorktree))
        .collect();
    let retry = retry_publication(&overflowing);
    assert_conflict(&overflowing, &overflow_worktree, &retry);

    let (mut overflowing, overflow_worktree) = claimed(
        AgentWorkStep::AddWorktree,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    overflowing.work.cleanup_debt = (0..=MAX_AGENT_CLEANUP_DEBT)
        .map(|_| debt(&overflowing, AgentWorkStep::AddWorktree))
        .collect();
    let retry = retry_publication(&overflowing);
    assert_conflict(&overflowing, &overflow_worktree, &retry);

    let (mut overlapping, overlap_worktree) = claimed(
        AgentWorkStep::MaterializeHome,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    overlapping
        .work
        .receipts
        .push(receipt(&overlapping, AgentWorkStep::AddWorktree));
    overlapping
        .work
        .cleanup_debt
        .push(debt(&overlapping, AgentWorkStep::AddWorktree));
    let retry = retry_publication(&overlapping);
    assert_conflict(&overlapping, &overlap_worktree, &retry);
}

#[test]
fn durable_log_high_water_is_part_of_the_shared_contract() {
    let (current, worktree) = claimed(
        AgentWorkStep::PublishConnection,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    let mutation = completed_advance(&current, AgentWorkStep::None);
    assert_eq!(
        validate_agent_work_mutation(
            &current,
            &worktree,
            current.conversation.last_seq + 1,
            &mutation,
        ),
        Err(SandboxStoreError::SequenceConflict)
    );
}

#[test]
fn first_message_work_can_retire_after_runtime_progresses_the_conversation() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::DispatchFirstMessage,
        AgentStatus::Working,
        WorktreeStatus::Ready,
    );
    let mut first = FirstMessageRecord::new("creation-key", "Start here", NOW).unwrap();
    first
        .fence_dispatch_for(2, 1, fence(&current), NOW.into())
        .unwrap();
    first.mark_write_confirmed(NOW.into()).unwrap();
    current.conversation.first_message = Some(first);
    current.conversation.active_turn = Some(2);

    let mutation = completed_advance(&current, AgentWorkStep::None);
    assert_valid(&current, &worktree, &mutation);

    let mut invalid = current.clone();
    invalid.status = AgentStatus::Starting;
    let mutation = completed_advance(&invalid, AgentWorkStep::None);
    assert_conflict(&invalid, &worktree, &mutation);
}

fn claimed(
    step: AgentWorkStep,
    status: AgentStatus,
    worktree_status: WorktreeStatus,
) -> (Agent, Worktree) {
    let id = AgentId("agt_validator".into());
    let worktree_id = WorktreeId("wkt_validator".into());
    let mut agent = Agent {
        id: id.clone(),
        worktree_id: worktree_id.clone(),
        workspace_id: WorkspaceId("wks_validator".into()),
        sandbox_id: SandboxId("sbx_validator".into()),
        owner: SubjectId::new("owner"),
        creation_key: "create-validator".into(),
        creation_fingerprint: "c".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_validator".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("gpt-test".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: "Be exact.".into(),
            },
        },
        capabilities: crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 3,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(step, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 7,
    };
    match status {
        AgentStatus::Ready
        | AgentStatus::Working
        | AgentStatus::WaitingApproval
        | AgentStatus::Paused => {
            agent.conversation.conversation_id = Some("conversation-1".into());
            agent.conversation.runtime_generation = Some(1);
            agent.ready_at = Some(NOW.into());
        }
        AgentStatus::Stopped => {
            agent.conversation.conversation_id = Some("conversation-1".into());
            agent.conversation.runtime_generation = Some(1);
            agent.ready_at = Some(NOW.into());
            agent.stopped_at = Some(NOW.into());
            agent.stop_reason = Some(AgentStopReason::Owner);
        }
        AgentStatus::Failed => agent.failure = Some(typed("startup_failed")),
        _ => {}
    }
    if matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    ) {
        agent.requested_stop_reason = Some(AgentStopReason::Owner);
    }
    agent
        .work
        .claim("worker".into(), NOW, LEASE_END.into())
        .unwrap();
    let worktree = Worktree {
        id: worktree_id,
        workspace_id: agent.workspace_id.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        owner: agent.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "a".repeat(40),
        status: worktree_status,
        agent_id: Some(id),
        revision: 2,
    };
    (agent, worktree)
}

fn publication(
    current: &Agent,
    mut next: Agent,
    logs: Vec<AgentLogKind>,
    event: Option<SandboxEventKind>,
    worktree: Option<VersionedWorktree>,
) -> AgentWorkMutation {
    next.revision = current.revision + 1;
    next.updated_at = AT.into();
    next.conversation.last_seq = current.conversation.last_seq + logs.len() as u64;
    AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence: fence(current),
        agent: next,
        worktree,
        appended_at: AT.into(),
        sandbox_event: event,
        agent_logs: logs,
    }
}

fn completed_advance(current: &Agent, next_step: AgentWorkStep) -> AgentWorkMutation {
    let mut next = current.clone();
    next.work
        .advance("worker", fence(current), next_step, AT.into())
        .unwrap();
    publication(
        current,
        next,
        vec![completed(current.work.step)],
        None,
        None,
    )
}

fn retry_publication(current: &Agent) -> AgentWorkMutation {
    let mut next = current.clone();
    next.work
        .retry("worker", fence(current), RETRY_AT.into())
        .unwrap();
    publication(
        current,
        next,
        vec![failed(current.work.step, "retryable")],
        None,
        None,
    )
}

fn versioned(current: &Worktree, next: Worktree) -> VersionedWorktree {
    VersionedWorktree {
        expected_revision: current.revision,
        worktree: next,
    }
}

fn add_startup_receipts(agent: &mut Agent) {
    for step in [
        AgentWorkStep::AddWorktree,
        AgentWorkStep::MaterializeHome,
        AgentWorkStep::OpenConversation,
    ] {
        agent.work.receipts.push(receipt(agent, step));
    }
}

fn add_cleanup_receipts(agent: &mut Agent) {
    add_startup_receipts(agent);
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
    ] {
        agent.work.receipts.push(receipt(agent, step));
    }
}

fn debt(agent: &Agent, step: AgentWorkStep) -> AgentCleanupDebt {
    AgentCleanupDebt {
        step,
        operation_id: operation_id(agent, step),
        request_sha256: "a".repeat(64),
    }
}

fn receipt(agent: &Agent, step: AgentWorkStep) -> AgentWorkReceipt {
    AgentWorkReceipt {
        step,
        operation_id: operation_id(agent, step),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    }
}

fn operation_id(agent: &Agent, step: AgentWorkStep) -> String {
    match step {
        AgentWorkStep::AddWorktree => "worktree:project:wt-1:add:v1".into(),
        AgentWorkStep::MaterializeHome => format!("agent:{}:home:create:v1", agent.id),
        AgentWorkStep::OpenConversation => format!("agent:{}:open:v1", agent.id),
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => {
            format!("agent:{}:close:v1", agent.id)
        }
        AgentWorkStep::RemoveHome => format!("agent:{}:home:remove:v1", agent.id),
        AgentWorkStep::RemoveWorktree => "worktree:project:wt-1:remove:v1".into(),
        _ => "invalid".into(),
    }
}

fn completed(step: AgentWorkStep) -> AgentLogKind {
    AgentLogKind::WorkStepCompleted { step }
}

fn failed(step: AgentWorkStep, class: &str) -> AgentLogKind {
    AgentLogKind::WorkStepFailed {
        step,
        failure: typed(class),
    }
}

fn typed(class: &str) -> TypedFailure {
    TypedFailure {
        class: class.into(),
        detail: "durable lifecycle failure".into(),
    }
}

fn fence(agent: &Agent) -> u64 {
    agent.work.lease.as_ref().unwrap().fence
}

fn assert_valid(current: &Agent, worktree: &Worktree, mutation: &AgentWorkMutation) {
    assert_eq!(
        validate_agent_work_mutation(current, worktree, current.conversation.last_seq, mutation,),
        Ok(())
    );
}

fn assert_conflict(current: &Agent, worktree: &Worktree, mutation: &AgentWorkMutation) {
    assert_eq!(
        validate_agent_work_mutation(current, worktree, current.conversation.last_seq, mutation,),
        Err(SandboxStoreError::Conflict)
    );
}

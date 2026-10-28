use super::*;
use crate::sandbox::{
    AgentCleanupDebt, AgentStopReason, AgentWorkLease, FirstMessageFailure, FirstMessageRecord,
    QueuedAgentMessage,
};

fn first_message(state: FirstMessageDelivery) -> FirstMessageRecord {
    let mut first = FirstMessageRecord::new("original-request", "retained input", NOW).unwrap();
    if state == FirstMessageDelivery::Queued {
        return first;
    }
    first.fence_dispatch_for(8, 9, 6, NOW.into()).unwrap();
    match state {
        FirstMessageDelivery::DispatchFenced => {}
        FirstMessageDelivery::WriteConfirmed => first.mark_write_confirmed(NOW.into()).unwrap(),
        FirstMessageDelivery::ExecutionObserved => {
            first.mark_execution_observed(NOW.into()).unwrap();
        }
        FirstMessageDelivery::Completed => first.complete(NOW.into()).unwrap(),
        FirstMessageDelivery::Failed | FirstMessageDelivery::RecoveryRequired => {
            let failure =
                FirstMessageFailure::new("original-failure", "retained failure", false, NOW)
                    .unwrap();
            if state == FirstMessageDelivery::Failed {
                first.fail(failure).unwrap();
            } else {
                first.require_recovery(failure).unwrap();
            }
        }
        FirstMessageDelivery::Queued => unreachable!(),
    }
    first
}

pub(super) fn deleted_agent(index: usize, state: FirstMessageDelivery) -> Agent {
    let mut deleted = agent(
        &format!("agt_deleted_{index}"),
        &format!("original-{index}"),
        10,
    );
    deleted.status = AgentStatus::Deleted;
    deleted.deleted_at = Some(NOW.into());
    deleted.stopped_at = Some(NOW.into());
    deleted.stop_reason = Some(AgentStopReason::Owner);
    deleted.conversation.runtime_generation = Some(1);
    deleted.conversation.first_message = Some(first_message(state));
    deleted
}

pub(super) fn terminal_snapshot() -> SandboxUnpublishedRecoverySnapshot {
    let mut held = snapshot(false);
    held.sandbox.lifecycle.runtime_generation_high_water = 3;
    held.sandbox.runtime.as_mut().unwrap().provision_generation = Some(3);
    held.agents = vec![
        deleted_agent(0, FirstMessageDelivery::WriteConfirmed),
        deleted_agent(1, FirstMessageDelivery::Queued),
    ];
    // Deletion retains interrupted-turn history; it cannot dispatch again.
    held.agents[0].conversation.active_turn = Some(8);
    held
}

#[test]
fn production_shape_admits_older_publication_without_process_receipts() {
    let mut held = snapshot(false);
    held.sandbox.lifecycle.runtime_generation_high_water = 5;
    held.sandbox.runtime.as_mut().unwrap().provision_generation = Some(5);
    let live = &mut held.agents[0];
    live.conversation.runtime_generation = Some(4);
    live.conversation.first_message = Some(first_message(FirstMessageDelivery::Completed));
    live.work.receipts = [AgentWorkStep::MaterializeHome, AgentWorkStep::AddWorktree]
        .into_iter()
        .map(|step| AgentWorkReceipt {
            step,
            operation_id: live.operation_id(step),
            request_sha256: "d".repeat(64),
            completed_at: NOW.into(),
        })
        .collect();
    held.agents.extend((0..49).map(|index| {
        deleted_agent(
            index,
            match index {
                0 => FirstMessageDelivery::WriteConfirmed,
                1 | 2 => FirstMessageDelivery::Queued,
                _ => FirstMessageDelivery::Completed,
            },
        )
    }));
    held.validate().unwrap();
    let requested = request(held.clone());
    assert_eq!(requested.proof.conversations.len(), 1);
    let mutation = apply_unpublished_runtime_recovery(&held, &requested).unwrap();
    assert_eq!(requested.snapshot.agents, held.agents);
    assert_eq!(mutation.sandbox.lifecycle.home, held.sandbox.lifecycle.home);
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        mutation.sandbox.lifecycle.operation
    else {
        panic!("recovery must retain its original live conversation");
    };
    assert_eq!(identity.agents.len(), 1);
    assert_eq!(identity.agents[0].agent_id, held.agents[0].id);
    assert_eq!(identity.agents[0].conversation_id, "native-original");
}

#[test]
fn live_or_stopped_unfinished_dispatch_remains_a_refusal_for_every_work_fence() {
    for status in [AgentStatus::Paused, AgentStatus::Stopped] {
        for state in [
            FirstMessageDelivery::DispatchFenced,
            FirstMessageDelivery::WriteConfirmed,
            FirstMessageDelivery::ExecutionObserved,
            FirstMessageDelivery::RecoveryRequired,
        ] {
            for fence in [1, 2, 6] {
                let mut held = snapshot(false);
                held.agents[0].status = status;
                let mut first = first_message(state);
                first.dispatch_generation = fence;
                held.agents[0].conversation.first_message = Some(first);
                held.agents[0].validate_conversation().unwrap();
                assert!(
                    held.validate().is_err(),
                    "{status:?}, {state:?}, fence {fence}"
                );
            }
        }
    }
    for state in [
        FirstMessageDelivery::Completed,
        FirstMessageDelivery::Failed,
        FirstMessageDelivery::Queued,
    ] {
        let mut held = snapshot(false);
        held.agents[0].work.receipts.clear();
        held.agents[0].conversation.first_message = Some(first_message(state));
        held.validate().unwrap();
    }
}

#[test]
fn deleted_delivery_history_requires_settled_deletion_and_older_publication() {
    for case in 0..11 {
        let mut held = terminal_snapshot();
        held.validate().unwrap();
        let deleted = &mut held.agents[0];
        match case {
            0 => deleted.deleted_at = None,
            1 => deleted.deleted_at = Some("invalid".into()),
            2 => deleted.work.cleanup_debt.push(AgentCleanupDebt {
                step: AgentWorkStep::RemoveWorktree,
                operation_id: deleted.operation_id(AgentWorkStep::RemoveWorktree),
                request_sha256: "a".repeat(64),
            }),
            3 => deleted.work.schedule(AgentWorkStep::RemoveHome, NOW.into()),
            4 => {
                deleted.work.lease = Some(AgentWorkLease {
                    holder: HOLDER.into(),
                    fence: 7,
                    expires_at: END.into(),
                })
            }
            5 => deleted.requested_stop_reason = Some(AgentStopReason::Owner),
            6 => deleted
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(11, "pending work".into(), NOW.into()).unwrap()),
            7 => deleted.conversation.runtime_generation = Some(3),
            8 | 9 => {
                let mut operation = AgentOperationRecord::new(
                    1,
                    if case == 8 { 1 } else { 3 },
                    2,
                    AgentOperationIntent::send_message("admitted work"),
                    NOW.into(),
                )
                .unwrap();
                if case == 9 {
                    operation
                        .retire_lost_runtime(
                            crate::sandbox::AgentOperationFailure::new(
                                "runtime_lost",
                                "retired operation",
                                NOW,
                            )
                            .unwrap(),
                        )
                        .unwrap();
                }
                deleted.conversation.operation = Some(operation);
            }
            10 => deleted.status = AgentStatus::Stopped,
            _ => unreachable!(),
        }
        assert!(held.validate().is_err(), "case {case}");
    }
}

#[test]
fn terminal_only_recovery_requires_original_boot_and_provider_proof() {
    let held = terminal_snapshot();
    let requested = request(held.clone());
    assert!(requested.proof.conversations.is_empty());
    assert!(requested.proof.boot_id.is_some());
    apply_unpublished_runtime_recovery(&held, &requested).unwrap();
    for case in 0..4 {
        let mut changed = requested.clone();
        match case {
            0 => {
                changed.proof.boot_id = None;
                changed.proof.runtime_process_absent = true;
            }
            1 => changed.proof.provider_plan = serde_json::Value::Null,
            2 => changed.proof.requires_native_quiescence = false,
            3 => changed
                .proof
                .conversations
                .push(SandboxRecoveryConversationProof {
                    agent_id: held.agents[0].id.clone(),
                    conversation_id: held.agents[0].conversation.conversation_id.clone().unwrap(),
                    history_sha256: "e".repeat(64),
                }),
            _ => unreachable!(),
        }
        assert!(
            apply_unpublished_runtime_recovery(&held, &changed).is_err(),
            "case {case}"
        );
    }
}

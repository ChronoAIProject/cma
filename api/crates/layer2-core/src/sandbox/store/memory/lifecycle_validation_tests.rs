use super::*;

use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use crate::sandbox::queued_message::queued_message_origin;
use crate::sandbox::store::{
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard,
};
use crate::sandbox::{
    AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord, AgentStatus,
    AgentWorkReceipt, AgentWorkStep, FirstMessageRecord, QueuedAgentMessage, SandboxEventKind,
    SandboxLifecycleOperation, SandboxPausePhase, SandboxReviveCause, SandboxRevivePhase,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

const LATER: &str = "2026-09-05T00:31:00Z";
const LEASE_END: &str = "2026-09-05T00:45:00Z";

#[tokio::test]
async fn sandbox_only_phase_rejects_agent_mutation_atomically() {
    let current = claimed_pause_phase(SandboxPausePhase::TeardownRuntime);
    let mut parked = test_agent("agt_sandbox_only", AgentStatus::Paused);
    parked.conversation.last_seq = 0;
    let fixture = seed_fixture(current, vec![parked.clone()]).await;
    let mut next_sandbox = fixture.current.clone();
    let lease = lifecycle_lease(&fixture.current);
    next_sandbox
        .complete_lifecycle_runtime_teardown(&lease.holder, lease.fence, LATER.into())
        .unwrap();
    let mut changed = parked;
    changed.revision += 1;
    changed.updated_at = LATER.into();
    let mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&fixture.current),
        sandbox: next_sandbox,
        agents: vec![SandboxLifecycleAgentMutation {
            expected_revision: 1,
            agent: changed,
            logs: Vec::new(),
        }],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: LATER.into(),
        events: vec![work_completed()],
    };

    assert_rejected_atomically(&fixture, mutation).await;
}

#[tokio::test]
async fn pause_rejects_partial_scope_wrong_status_and_extra_logs() {
    let current = claimed_pause_phase(SandboxPausePhase::PauseAgents);
    let agents = vec![
        test_agent("agt_pause_a", AgentStatus::Ready),
        test_agent("agt_pause_b", AgentStatus::Ready),
    ];
    let fixture = seed_fixture(current, agents).await;
    let valid = pause_mutation(&fixture.current, &fixture.agents);

    let mut partial = valid.clone();
    partial.agents.pop();
    partial.events.remove(1);
    assert_rejected_atomically(&fixture, partial).await;

    let mut wrong_status = valid.clone();
    wrong_status.agents[0].agent.status = AgentStatus::Ready;
    assert_rejected_atomically(&fixture, wrong_status).await;

    let mut extra_log = valid;
    extra_log.agents[0].logs.push(AgentLogKind::OwnerMessage {
        content: "extra".into(),
    });
    extra_log.agents[0].agent.conversation.last_seq += 1;
    assert_rejected_atomically(&fixture, extra_log).await;
}

#[tokio::test]
async fn pause_retries_fence_already_paused_agents_without_duplicate_status_logs() {
    let current = claimed_pause_phase(SandboxPausePhase::PauseAgents);
    let agents = vec![
        test_agent("agt_ready", AgentStatus::Ready),
        test_agent("agt_parked", AgentStatus::Paused),
    ];
    let fixture = seed_fixture(current, agents).await;
    let valid = pause_mutation(&fixture.current, &fixture.agents);
    let mut omitted = valid.clone();
    omitted.agents.pop();
    assert_rejected_atomically(&fixture, omitted).await;
    let mut duplicate = valid.clone();
    duplicate.agents[1].logs.push(AgentLogKind::StatusChanged {
        from: AgentStatus::Paused,
        to: AgentStatus::Paused,
    });
    duplicate.agents[1].agent.conversation.last_seq += 1;
    duplicate.events.insert(
        1,
        SandboxEventKind::AgentStatusChanged {
            agent_id: fixture.agents[1].id.clone(),
            from: AgentStatus::Paused,
            to: AgentStatus::Paused,
        },
    );
    assert_rejected_atomically(&fixture, duplicate).await;
    fixture
        .store
        .commit_sandbox_lifecycle(valid.clone())
        .await
        .unwrap();
    for change in &valid.agents {
        assert_eq!(
            fixture.store.get_agent(&change.agent.id).await.unwrap(),
            Some(change.agent.clone())
        );
    }
    assert!(fixture
        .store
        .agent_log(&fixture.agents[1].id, 0, 100)
        .await
        .unwrap()
        .is_empty());
    let events = fixture
        .store
        .sandbox_events(&fixture.current.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        valid.events
    );
}

#[tokio::test]
async fn reopen_rejects_nonincreasing_generation_and_changed_conversation_identity() {
    let current = claimed_revive_phase(SandboxRevivePhase::ConversationReopen);
    let agents = vec![test_agent("agt_reopen", AgentStatus::Paused)];
    let fixture = seed_fixture(current, agents).await;
    let valid = reopen_mutation(&fixture.current, &fixture.agents);

    let mut unchanged_generation = valid.clone();
    unchanged_generation.agents[0]
        .agent
        .conversation
        .runtime_generation = Some(4);
    unchanged_generation.agents[0].logs[0] = AgentLogKind::ConversationOpened {
        conversation_id: "conversation-agt_reopen".into(),
        runtime_generation: 4,
    };
    assert_rejected_atomically(&fixture, unchanged_generation).await;

    let mut unbound_generation = valid.clone();
    unbound_generation.agents[0]
        .agent
        .conversation
        .runtime_generation = Some(6);
    unbound_generation.agents[0].logs[0] = AgentLogKind::ConversationOpened {
        conversation_id: "conversation-agt_reopen".into(),
        runtime_generation: 6,
    };
    assert_rejected_atomically(&fixture, unbound_generation).await;

    let mut changed_identity = valid.clone();
    changed_identity.agents[0]
        .agent
        .conversation
        .conversation_id = Some("different-conversation".into());
    changed_identity.agents[0].logs[0] = AgentLogKind::ConversationOpened {
        conversation_id: "different-conversation".into(),
        runtime_generation: 5,
    };
    assert_rejected_atomically(&fixture, changed_identity).await;
    fixture.store.commit_sandbox_lifecycle(valid).await.unwrap();
}

#[tokio::test]
async fn reopen_requires_exact_receipt_publication_and_preserves_other_work() {
    let current = claimed_revive_phase(SandboxRevivePhase::ConversationReopen);
    let fixture = seed_fixture(
        current,
        vec![test_agent("agt_receipt", AgentStatus::Paused)],
    )
    .await;
    let valid = reopen_mutation(&fixture.current, &fixture.agents);
    for invalid in [
        "missing",
        "operation",
        "digest",
        "time",
        "duplicate",
        "work",
    ] {
        let mut mutation = valid.clone();
        let work = &mut mutation.agents[0].agent.work;
        match invalid {
            "missing" => work.receipts.clear(),
            "operation" => work.receipts[0].operation_id = "another-agent:open:v1".into(),
            "digest" => work.receipts[0].request_sha256 = "invalid".into(),
            "time" => work.receipts[0].completed_at = NOW.into(),
            "duplicate" => work.receipts.push(work.receipts[0].clone()),
            "work" => work.next_fence += 1,
            _ => unreachable!(),
        }
        assert_rejected_atomically(&fixture, mutation).await;
    }
    fixture
        .store
        .commit_sandbox_lifecycle(valid.clone())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .get_agent(&fixture.agents[0].id)
            .await
            .unwrap(),
        Some(valid.agents[0].agent.clone())
    );
}

#[tokio::test]
async fn restore_scope_defers_only_unstarted_reservations_and_preserves_all_live_semantics() {
    let current = claimed_revive_phase(SandboxRevivePhase::ConversationReopen);
    let restored = test_agent("agt_restore_existing", AgentStatus::Paused);
    let mut fresh = test_agent("agt_restore_new", AgentStatus::Starting);
    fresh.ready_at = None;
    fresh.conversation.conversation_id = None;
    fresh.conversation.runtime_generation = None;
    fresh.work =
        crate::sandbox::AgentWorkState::new(crate::sandbox::AgentWorkStep::AddWorktree, NOW.into());
    let fixture = seed_fixture(current.clone(), vec![restored.clone(), fresh.clone()]).await;
    let mut mutation = reopen_mutation(&current, std::slice::from_ref(&restored));
    assert_rejected_atomically(&fixture, mutation.clone()).await;
    mutation.agent_scope = SandboxLifecycleAgentScope::RestoreParticipants;
    fixture
        .store
        .commit_sandbox_lifecycle(mutation.clone())
        .await
        .unwrap();
    assert_eq!(
        fixture.store.get_agent(&fresh.id).await.unwrap(),
        Some(fresh.clone())
    );

    fresh
        .work
        .cleanup_debt
        .push(crate::sandbox::AgentCleanupDebt {
            step: crate::sandbox::AgentWorkStep::AddWorktree,
            operation_id: "worktree-intent".into(),
            request_sha256: "a".repeat(64),
        });
    let admitted_effect = seed_fixture(current, vec![restored, fresh]).await;
    assert_rejected_atomically(&admitted_effect, mutation).await;
}

#[tokio::test]
async fn queue_progress_rejects_non_fifo_selection_and_extra_owner_log() {
    let current = claimed_revive_phase(SandboxRevivePhase::QueuedMessageDelivery);
    let queued = queued_agent("agt_queue", &["first", "second"]);
    let fixture = seed_fixture(current, vec![queued]).await;

    let non_fifo = queue_progress_mutation(&fixture.current, &fixture.agents[0], 1);
    assert_rejected_atomically(&fixture, non_fifo).await;

    let mut extra_owner = queue_progress_mutation(&fixture.current, &fixture.agents[0], 0);
    extra_owner.agents[0].logs.push(AgentLogKind::OwnerMessage {
        content: "duplicate".into(),
    });
    extra_owner.agents[0].agent.conversation.last_seq += 1;
    assert_rejected_atomically(&fixture, extra_owner).await;
}

#[tokio::test]
async fn queue_progress_rejects_unrelated_queue_operation_turn_and_status_changes() {
    let current = claimed_revive_phase(SandboxRevivePhase::QueuedMessageDelivery);
    let queued = queued_agent("agt_queue_shape", &["first", "second"]);
    let fixture = seed_fixture(current, vec![queued]).await;
    let valid = queue_progress_mutation(&fixture.current, &fixture.agents[0], 0);

    let mut queue_changed = valid.clone();
    queue_changed.agents[0]
        .agent
        .conversation
        .queued_messages
        .push(QueuedAgentMessage::new(99, "unrelated".into(), NOW.into()).unwrap());
    assert_rejected_atomically(&fixture, queue_changed).await;

    let mut operation_changed = valid.clone();
    operation_changed.agents[0]
        .agent
        .conversation
        .operation
        .as_mut()
        .unwrap()
        .intent = AgentOperationIntent::send_message("second");
    assert_rejected_atomically(&fixture, operation_changed).await;

    let mut turn_changed = valid.clone();
    turn_changed.agents[0].agent.conversation.active_turn = Some(99);
    assert_rejected_atomically(&fixture, turn_changed).await;

    let mut status_changed = valid;
    status_changed.agents[0].agent.status = AgentStatus::Ready;
    status_changed.agents[0].agent.conversation.active_turn = None;
    assert_rejected_atomically(&fixture, status_changed).await;
}

#[tokio::test]
async fn queue_completion_rejects_partial_live_scope() {
    let current = claimed_revive_phase(SandboxRevivePhase::QueuedMessageDelivery);
    let agents = vec![
        delivered_agent("agt_complete_a"),
        test_agent("agt_complete_b", AgentStatus::Ready),
    ];
    let fixture = seed_fixture(current, agents).await;
    let mut mutation = queue_completion_mutation(&fixture.current, &fixture.agents);
    mutation.agents.pop();

    assert_rejected_atomically(&fixture, mutation).await;
}

#[tokio::test]
async fn queue_completion_rejects_unsettled_agent_states() {
    let queued = queued_agent("agt_unsettled_queue", &["pending"]);
    assert_queue_completion_rejected(queued).await;

    let mut blocking = queued_agent("agt_unsettled_operation", &["pending"]);
    let message = &mut blocking.conversation.queued_messages[0];
    message.fence_dispatch(LATER.into()).unwrap();
    blocking.conversation.operation = Some(
        AgentOperationRecord::new(
            2,
            4,
            3,
            AgentOperationIntent::send_message("pending"),
            LATER.into(),
        )
        .unwrap(),
    );
    assert_queue_completion_rejected(blocking).await;

    let mut active = delivered_agent("agt_unsettled_turn");
    active.status = AgentStatus::Working;
    active.conversation.active_turn = Some(2);
    assert_queue_completion_rejected(active).await;
}

#[tokio::test]
async fn queue_completion_requires_durable_delivery_and_terminal_first_message() {
    let mut no_delivery = delivered_agent("agt_no_delivery");
    no_delivery.conversation.operation = None;
    assert_queue_completion_rejected(no_delivery).await;

    let mut wrong_fence = delivered_agent("agt_wrong_delivery_fence");
    wrong_fence
        .conversation
        .operation
        .as_mut()
        .unwrap()
        .dispatch_fenced_at = NOW.into();
    assert_queue_completion_rejected(wrong_fence).await;

    let mut first_message = test_agent("agt_first_message", AgentStatus::Ready);
    first_message.conversation.first_message =
        Some(FirstMessageRecord::new("origin", "initial", NOW).unwrap());
    assert_queue_completion_rejected(first_message).await;
}

#[tokio::test]
async fn final_publication_rejects_concurrent_work_queue_and_first_message() {
    let mut working = test_agent("agt_publish_working", AgentStatus::Working);
    working.conversation.active_turn = Some(1);
    assert_publish_active_rejected(working).await;

    let queued = queued_agent("agt_publish_queued", &["late"]);
    assert_publish_active_rejected(queued).await;

    let mut first_message = test_agent("agt_publish_first", AgentStatus::Ready);
    first_message.conversation.first_message =
        Some(FirstMessageRecord::new("origin", "initial", NOW).unwrap());
    assert_publish_active_rejected(first_message).await;
}

#[tokio::test]
async fn lifecycle_events_reject_missing_duplicate_and_unrelated_entries() {
    let current = claimed_pause_phase(SandboxPausePhase::PauseAgents);
    let agents = vec![test_agent("agt_events", AgentStatus::Ready)];
    let fixture = seed_fixture(current, agents).await;
    let valid = pause_mutation(&fixture.current, &fixture.agents);

    let mut missing = valid.clone();
    missing.events.pop();
    assert_rejected_atomically(&fixture, missing).await;

    let mut duplicate = valid.clone();
    duplicate.events.push(duplicate.events[0].clone());
    assert_rejected_atomically(&fixture, duplicate).await;

    let mut unrelated = valid;
    unrelated.events[0] = SandboxEventKind::CheckpointSealed {
        object_reference: "unrelated".into(),
    };
    assert_rejected_atomically(&fixture, unrelated).await;

    let current = claimed_revive_phase(SandboxRevivePhase::PublishActive);
    let agents = vec![test_agent("agt_final_events", AgentStatus::Ready)];
    let fixture = seed_fixture(current, agents).await;
    let valid = publish_active_mutation(&fixture.current);

    let mut final_missing = valid.clone();
    final_missing.events.pop();
    assert_rejected_atomically(&fixture, final_missing).await;

    let mut final_duplicate = valid.clone();
    final_duplicate.events.push(SandboxEventKind::Revived);
    assert_rejected_atomically(&fixture, final_duplicate).await;

    let mut final_unrelated = valid;
    final_unrelated.events[1] = work_completed();
    assert_rejected_atomically(&fixture, final_unrelated).await;
}

struct ValidationFixture {
    store: InMemorySandboxStore,
    current: Sandbox,
    agents: Vec<Agent>,
}

async fn seed_fixture(current: Sandbox, agents: Vec<Agent>) -> ValidationFixture {
    let store = InMemorySandboxStore::default();
    assert_eq!(current.revision, 1);
    store.create_sandbox(current.clone()).await.unwrap();
    for value in &agents {
        assert_eq!(value.revision, 1);
        store.create_agent(value.clone()).await.unwrap();
        for sequence in 1..=value.conversation.last_seq {
            store
                .append_agent_log(
                    &value.id,
                    NOW.into(),
                    AgentLogKind::AgentMessage {
                        content: format!("seed-{sequence}"),
                    },
                )
                .await
                .unwrap();
        }
    }
    ValidationFixture {
        store,
        current,
        agents,
    }
}

async fn assert_rejected_atomically(
    fixture: &ValidationFixture,
    mutation: SandboxLifecycleMutation,
) {
    let before_sandbox = fixture
        .store
        .get_sandbox(&fixture.current.id)
        .await
        .unwrap();
    let mut before_agents = Vec::new();
    let mut before_logs = Vec::new();
    for agent in &fixture.agents {
        before_agents.push(fixture.store.get_agent(&agent.id).await.unwrap());
        before_logs.push(fixture.store.agent_log(&agent.id, 0, 100).await.unwrap());
    }
    let before_events = fixture
        .store
        .sandbox_events(&fixture.current.id, 0, 100)
        .await
        .unwrap();

    assert!(fixture
        .store
        .commit_sandbox_lifecycle(mutation)
        .await
        .is_err());
    assert_eq!(
        fixture
            .store
            .get_sandbox(&fixture.current.id)
            .await
            .unwrap(),
        before_sandbox
    );
    for (index, agent) in fixture.agents.iter().enumerate() {
        assert_eq!(
            fixture.store.get_agent(&agent.id).await.unwrap(),
            before_agents[index]
        );
        assert_eq!(
            fixture.store.agent_log(&agent.id, 0, 100).await.unwrap(),
            before_logs[index]
        );
    }
    assert_eq!(
        fixture
            .store
            .sandbox_events(&fixture.current.id, 0, 100)
            .await
            .unwrap(),
        before_events
    );
}

fn claimed_pause_phase(phase: SandboxPausePhase) -> Sandbox {
    let mut value = sandbox();
    value.begin_pause(NOW.into(), None).unwrap();
    value
        .lifecycle
        .checkpoints
        .publish(checkpoint(1, NOW))
        .unwrap();
    let Some(SandboxLifecycleOperation::Pause {
        phase: current,
        checkpoint_epoch,
        ..
    }) = value.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *current = phase;
    *checkpoint_epoch = Some(1);
    value
        .work
        .claim("lifecycle-worker".into(), NOW, LEASE_END.into())
        .unwrap();
    value.revision = 1;
    value.validate().unwrap();
    value
}

fn claimed_revive_phase(phase: SandboxRevivePhase) -> Sandbox {
    let mut value = sandbox();
    let runtime = value.runtime.take().unwrap();
    value
        .lifecycle
        .checkpoints
        .publish(checkpoint(1, NOW))
        .unwrap();
    value.status = SandboxStatus::Paused;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    value
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();
    let Some(SandboxLifecycleOperation::Revive { phase: current, .. }) =
        value.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *current = phase;
    value.runtime = Some(runtime);
    value.runtime.as_mut().unwrap().provision_generation = Some(5);
    value.lifecycle.runtime_generation_high_water = 5;
    value
        .work
        .claim("lifecycle-worker".into(), NOW, LEASE_END.into())
        .unwrap();
    value.revision = 1;
    value.validate().unwrap();
    value
}

fn test_agent(id: &str, status: AgentStatus) -> Agent {
    let mut value = agent(id, &format!("conversation-{id}"), 0);
    value.status = status;
    value.revision = 1;
    value
}

fn queued_agent(id: &str, contents: &[&str]) -> Agent {
    let mut value = test_agent(id, AgentStatus::Ready);
    value.conversation.queued_messages = contents
        .iter()
        .enumerate()
        .map(|(index, content)| {
            QueuedAgentMessage::new(
                u64::try_from(index).unwrap() + 1,
                (*content).into(),
                NOW.into(),
            )
            .unwrap()
        })
        .collect();
    value.conversation.last_seq = u64::try_from(contents.len()).unwrap();
    value
}

fn delivered_agent(id: &str) -> Agent {
    let mut value = queued_agent(id, &["delivered"]);
    value.conversation.queued_messages[0]
        .fence_dispatch(LATER.into())
        .unwrap();
    value.conversation.queued_messages[0]
        .mark_delivered()
        .unwrap();
    let mut operation = AgentOperationRecord::new(
        1,
        4,
        2,
        AgentOperationIntent::send_message("delivered"),
        LATER.into(),
    )
    .unwrap();
    operation.mark_write_confirmed(LATER.into()).unwrap();
    value.conversation.operation = Some(operation);
    value
}

fn pause_mutation(current: &Sandbox, agents: &[Agent]) -> SandboxLifecycleMutation {
    let mut next_sandbox = current.clone();
    let lease = lifecycle_lease(current);
    next_sandbox
        .complete_lifecycle_agent_pause(&lease.holder, lease.fence, LATER.into())
        .unwrap();
    let mut events = Vec::new();
    let changes = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            if current.status == AgentStatus::Paused {
                next.revision += 1;
                next.updated_at = LATER.into();
                return SandboxLifecycleAgentMutation {
                    expected_revision: current.revision,
                    agent: next,
                    logs: Vec::new(),
                };
            }
            next.transition(AgentStatus::Paused, LATER.into(), None)
                .unwrap();
            next.conversation.last_seq += 1;
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Paused,
            });
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: vec![AgentLogKind::StatusChanged {
                    from: current.status,
                    to: AgentStatus::Paused,
                }],
            }
        })
        .collect();
    events.push(work_completed());
    lifecycle_mutation(
        current,
        next_sandbox,
        changes,
        SandboxLifecycleAgentScope::AllLive,
        events,
    )
}

fn reopen_mutation(current: &Sandbox, agents: &[Agent]) -> SandboxLifecycleMutation {
    let mut next_sandbox = current.clone();
    let lease = lifecycle_lease(current);
    next_sandbox
        .complete_lifecycle_conversation_reopen(&lease.holder, lease.fence, LATER.into())
        .unwrap();
    let mut events = Vec::new();
    let changes = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            let conversation_id = current.conversation.conversation_id.clone().unwrap();
            next.conversation.runtime_generation = Some(5);
            next.work.receipts.push(AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: format!("agent:{}:open:v1", current.id),
                request_sha256: "a".repeat(64),
                completed_at: LATER.into(),
            });
            next.transition(AgentStatus::Ready, LATER.into(), None)
                .unwrap();
            next.conversation.last_seq += 2;
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: AgentStatus::Paused,
                to: AgentStatus::Ready,
            });
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: vec![
                    AgentLogKind::ConversationOpened {
                        conversation_id,
                        runtime_generation: 5,
                    },
                    AgentLogKind::StatusChanged {
                        from: AgentStatus::Paused,
                        to: AgentStatus::Ready,
                    },
                ],
            }
        })
        .collect();
    events.push(work_completed());
    lifecycle_mutation(
        current,
        next_sandbox,
        changes,
        SandboxLifecycleAgentScope::AllLive,
        events,
    )
}

fn queue_progress_mutation(
    current: &Sandbox,
    agent: &Agent,
    queue_index: usize,
) -> SandboxLifecycleMutation {
    let mut next_sandbox = current.clone();
    let lease = lifecycle_lease(current);
    next_sandbox
        .publish_lifecycle_queue_progress(&lease.holder, lease.fence, LATER.into())
        .unwrap();
    let mut next = agent.clone();
    let queued = &agent.conversation.queued_messages[queue_index];
    next.conversation.queued_messages[queue_index]
        .fence_dispatch(LATER.into())
        .unwrap();
    let operation_sequence = agent.conversation.last_seq + 1;
    let turn_sequence = operation_sequence + 1;
    next.conversation.operation = Some(
        AgentOperationRecord::new(
            operation_sequence,
            4,
            turn_sequence,
            AgentOperationIntent::send_message(&queued.content),
            LATER.into(),
        )
        .unwrap(),
    );
    next.conversation.active_turn = Some(turn_sequence);
    next.transition(AgentStatus::Working, LATER.into(), None)
        .unwrap();
    next.conversation.last_seq += 3;
    lifecycle_mutation(
        current,
        next_sandbox,
        vec![SandboxLifecycleAgentMutation {
            expected_revision: agent.revision,
            agent: next,
            logs: vec![
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence,
                    operation: crate::sandbox::AgentOperationKind::SendMessage,
                    state: AgentOperationDelivery::DispatchFenced,
                },
                AgentLogKind::TurnStarted {
                    model: Some(agent.settings().model),
                    turn_sequence,
                    origin_key: Some(queued_message_origin(queued.sequence)),
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Ready,
                    to: AgentStatus::Working,
                },
            ],
        }],
        SandboxLifecycleAgentScope::Touched,
        vec![
            SandboxEventKind::AgentStatusChanged {
                agent_id: agent.id.clone(),
                from: AgentStatus::Ready,
                to: AgentStatus::Working,
            },
            work_completed(),
        ],
    )
}

fn queue_completion_mutation(current: &Sandbox, agents: &[Agent]) -> SandboxLifecycleMutation {
    let mut next_sandbox = current.clone();
    let lease = lifecycle_lease(current);
    next_sandbox
        .complete_lifecycle_queued_message_delivery(&lease.holder, lease.fence, LATER.into())
        .unwrap();
    let changes = agents
        .iter()
        .map(|current| {
            let mut next = current.clone();
            next.conversation.queued_messages.clear();
            next.revision += 1;
            next.updated_at = LATER.into();
            SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent: next,
                logs: Vec::new(),
            }
        })
        .collect();
    lifecycle_mutation(
        current,
        next_sandbox,
        changes,
        SandboxLifecycleAgentScope::AllLive,
        vec![work_completed()],
    )
}

fn publish_active_mutation(current: &Sandbox) -> SandboxLifecycleMutation {
    let mut next = current.clone();
    let lease = lifecycle_lease(current);
    next.publish_lifecycle_active(LEASE_END.into(), &lease.holder, lease.fence, LATER.into())
        .unwrap();
    lifecycle_mutation(
        current,
        next,
        Vec::new(),
        SandboxLifecycleAgentScope::Touched,
        vec![
            SandboxEventKind::StatusChanged {
                from: SandboxStatus::Reviving,
                to: SandboxStatus::Active,
            },
            SandboxEventKind::Revived,
        ],
    )
}

fn lifecycle_mutation(
    current: &Sandbox,
    sandbox: Sandbox,
    agents: Vec<SandboxLifecycleAgentMutation>,
    agent_scope: SandboxLifecycleAgentScope,
    events: Vec<SandboxEventKind>,
) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox,
        agents,
        agent_scope,
        appended_at: LATER.into(),
        events,
    }
}

async fn assert_queue_completion_rejected(agent: Agent) {
    let current = claimed_revive_phase(SandboxRevivePhase::QueuedMessageDelivery);
    let fixture = seed_fixture(current, vec![agent]).await;
    let mutation = queue_completion_mutation(&fixture.current, &fixture.agents);
    assert_rejected_atomically(&fixture, mutation).await;
}

async fn assert_publish_active_rejected(agent: Agent) {
    let current = claimed_revive_phase(SandboxRevivePhase::PublishActive);
    let fixture = seed_fixture(current, vec![agent]).await;
    let mutation = publish_active_mutation(&fixture.current);
    assert_rejected_atomically(&fixture, mutation).await;
}

fn lifecycle_lease(sandbox: &Sandbox) -> crate::sandbox::SandboxWorkLease {
    sandbox.work.lease.clone().unwrap()
}

fn work_completed() -> SandboxEventKind {
    SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    }
}

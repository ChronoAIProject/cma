use std::sync::Arc;

use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, MessageId, PermissionLocator, PermissionOption,
    PermissionOptionKind, PermissionRequest, PromptResponse, RequestPermissionRequest,
    SessionUpdate, ToolCallUpdate, ToolCallUpdateFields, TurnStopReason,
};

use crate::sandbox::store::{
    AgentAdmissionStore, AgentConversationMutation, AgentConversationStore, AgentLogStore,
    AgentStore, AgentWorkMutation, AgentWorkerStore, SandboxMutationGuard, VersionedWorktree,
};
use crate::sandbox::{
    AgentConversationError, AgentLogEntry, AgentLogKind, AgentMessageAssembly, AgentStatus,
    AgentStopReason, AgentUpdateRecorder, AgentWorkReceipt, AgentWorkStep,
    DurableAgentUpdateRecorder, FirstMessageDelivery, FirstMessageRecord, SandboxEventKind,
    WorktreeStatus,
};

use super::agent_admission_tests::*;

#[path = "agent_live_tests.rs"]
mod live_tests;

#[path = "agent_tool_update_tests.rs"]
mod tool_update_tests;

#[path = "agent_capacity_planning_tests.rs"]
mod capacity_planning_tests;
#[path = "agent_message_recovery_tests.rs"]
mod message_recovery_tests;

#[tokio::test]
async fn accepted_text_publishes_before_terminal_and_survives_recorder_replacement() {
    for reason in [TurnStopReason::EndTurn, TurnStopReason::Cancelled] {
        let (store, agent) = ready_agent(None).await;
        let store = Arc::new(store);
        let mutation = begin_turn(&store, agent);
        let agent = mutation.agent.clone();
        store.commit_agent_conversation(mutation).await.unwrap();
        let recorder = DurableAgentUpdateRecorder::with_incremental_output_policy(
            store.clone(),
            Arc::default(),
            None,
            false,
            false,
            None,
            true,
        );
        let chunk = |text: &str| {
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from(text)).message_id(MessageId::new("answer")),
            ))
        };
        let first = recorder
            .record_update(
                &agent.id,
                1,
                &mut AgentMessageAssembly::default(),
                chunk("é"),
                NOW.into(),
            )
            .await
            .unwrap();
        assert!(!first.turn_closed);
        assert_eq!(first.appended.len(), 1);
        assert_eq!(
            first.appended[0].kind,
            AgentLogKind::AgentMessageDelta {
                offset: 0,
                content: "é".into(),
            }
        );
        let replacement = DurableAgentUpdateRecorder::new(store.clone());
        let mut assembly = AgentMessageAssembly::default();
        let second = replacement
            .record_update(&agent.id, 1, &mut assembly, chunk(" clair"), NOW.into())
            .await
            .unwrap();
        assert_eq!(second.appended.len(), 1);
        assert_eq!(
            second.appended[0].kind,
            AgentLogKind::AgentMessageDelta {
                offset: 2,
                content: " clair".into(),
            }
        );
        let terminal = replacement
            .record_update(
                &agent.id,
                1,
                &mut assembly,
                ConversationUpdate::TurnEnded(PromptResponse::new(reason)),
                NOW.into(),
            )
            .await
            .unwrap();
        assert!(terminal.turn_closed);
        let messages = store
            .agent_log(&agent.id, agent.conversation.last_seq, 50)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|row| match row.kind {
                AgentLogKind::AgentMessage { content } => Some(content),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(messages, ["é clair"]);
    }
}

async fn ready_agent(
    first_message: Option<&str>,
) -> (super::InMemorySandboxStore, crate::sandbox::Agent) {
    let store = ready_store(1).await;
    let mut request = admission("conversation");
    request.first_message = first_message.map(|text| {
        FirstMessageRecord::new("conversation", text, NOW).expect("valid first message")
    });
    let creation = created(store.admit_agent(request).await.unwrap());
    let mut claimed = store
        .claim_agent_work(
            &creation.agent.id,
            "worker",
            NOW.into(),
            "2026-09-05T00:01:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    claimed.work.step = AgentWorkStep::PublishReady;
    claimed.conversation.conversation_id = Some("conversation-1".into());
    claimed.conversation.runtime_generation = Some(1);
    claimed.work.receipts = [
        (
            AgentWorkStep::AddWorktree,
            "worktree:project:conversation:add:v1".into(),
        ),
        (
            AgentWorkStep::MaterializeHome,
            format!("agent:{}:home:create:v1", claimed.id),
        ),
        (
            AgentWorkStep::OpenConversation,
            format!("agent:{}:open:v1", claimed.id),
        ),
    ]
    .into_iter()
    .map(|(step, operation_id)| AgentWorkReceipt {
        step,
        operation_id,
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    })
    .collect();
    store
        .lock()
        .agents
        .insert(claimed.id.clone(), claimed.clone());
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut agent = claimed.clone();
    agent.conversation.last_seq = 3;
    agent
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    agent
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let mut worktree = creation.worktree;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: agent.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: agent.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: vec![
                AgentLogKind::ConversationOpened {
                    conversation_id: "conversation-1".into(),
                    runtime_generation: 1,
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
            ],
        })
        .await
        .unwrap();
    (store, agent)
}

fn begin_turn(
    store: &super::InMemorySandboxStore,
    mut agent: crate::sandbox::Agent,
) -> AgentConversationMutation {
    let admission_guard = Some(SandboxMutationGuard::capture(
        store.lock().sandboxes.get(&agent.sandbox_id).unwrap(),
    ));
    let expected_agent_revision = agent.revision;
    let expected_last_seq = agent.conversation.last_seq;
    let turn_sequence = expected_last_seq + 1;
    agent
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    agent.conversation.active_turn = Some(turn_sequence);
    agent.conversation.last_seq += 2;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard,
        expected_agent_revision,
        expected_last_seq,
        runtime_generation: 1,
        agent,
        appended_at: NOW.into(),
        agent_logs: vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect the proof.".into(),
            },
        ],
    }
}

fn begin_first_message(
    store: &super::InMemorySandboxStore,
    mut agent: crate::sandbox::Agent,
) -> AgentConversationMutation {
    let admission_guard = Some(SandboxMutationGuard::capture(
        store.lock().sandboxes.get(&agent.sandbox_id).unwrap(),
    ));
    let expected_agent_revision = agent.revision;
    let expected_last_seq = agent.conversation.last_seq;
    let fence_sequence = expected_last_seq + 1;
    let turn_sequence = expected_last_seq + 2;
    let first_message = agent.conversation.first_message.as_mut().unwrap();
    first_message
        .fence_dispatch(turn_sequence, fence_sequence, NOW.into())
        .unwrap();
    first_message.mark_write_confirmed(NOW.into()).unwrap();
    agent
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    agent.conversation.active_turn = Some(turn_sequence);
    agent.conversation.last_seq += 4;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard,
        expected_agent_revision,
        expected_last_seq,
        runtime_generation: 1,
        agent,
        appended_at: NOW.into(),
        agent_logs: vec![
            AgentLogKind::FirstMessageChanged {
                state: FirstMessageDelivery::DispatchFenced,
                recovery_generation: 0,
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: Some("conversation".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect the proof.".into(),
            },
            AgentLogKind::FirstMessageChanged {
                state: FirstMessageDelivery::WriteConfirmed,
                recovery_generation: 0,
            },
        ],
    }
}

async fn working_agent() -> (
    super::InMemorySandboxStore,
    crate::sandbox::Agent,
    Arc<DurableAgentUpdateRecorder>,
) {
    let (store, agent) = ready_agent(None).await;
    let mutation = begin_turn(&store, agent);
    let working = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    (store, working, recorder)
}

fn message_chunk(message_id: &str, content: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from(content))
            .message_id(MessageId::new(message_id.to_owned())),
    ))
}

fn turn_ended() -> ConversationUpdate {
    ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn))
}

#[tokio::test]
async fn conversation_mutation_updates_agent_and_log_atomically() {
    let (store, agent) = ready_agent(None).await;
    let mutation = begin_turn(&store, agent);
    let expected = mutation.agent.clone();

    let appended = store.commit_agent_conversation(mutation).await.unwrap();

    assert_eq!(
        appended.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [4, 5]
    );
    assert_eq!(store.get_agent(&expected.id).await.unwrap(), Some(expected));
}

#[tokio::test]
async fn stale_runtime_generation_cannot_write() {
    let (store, agent) = ready_agent(None).await;
    let mut mutation = begin_turn(&store, agent.clone());
    mutation.runtime_generation = 2;

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn late_sequence_failure_rolls_back_the_agent() {
    let (store, agent) = ready_agent(None).await;
    let mut mutation = begin_turn(&store, agent.clone());
    mutation.agent.conversation.last_seq += 1;

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn invalid_conversation_timestamp_is_rejected_atomically() {
    let (store, agent) = ready_agent(None).await;
    let mut mutation = begin_turn(&store, agent.clone());
    mutation.appended_at = "not-a-timestamp".into();

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn conversation_mutation_cannot_change_requested_stop_intent() {
    let (store, agent) = ready_agent(None).await;
    let mut mutation = begin_turn(&store, agent.clone());
    mutation.agent.requested_stop_reason = Some(AgentStopReason::Owner);

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn conversation_mutation_cannot_change_capability_grants() {
    let (store, agent) = ready_agent(None).await;
    let mut mutation = begin_turn(&store, agent.clone());
    mutation.agent.capabilities = crate::RuntimeCapabilities::default();

    assert_eq!(
        store.commit_agent_conversation(mutation).await.unwrap_err(),
        crate::sandbox::store::SandboxStoreError::Conflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn chunks_stay_live_until_complete_messages_can_be_committed_in_order() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();

    let first = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("message-1", "first "),
            NOW.into(),
        )
        .await
        .unwrap();
    let second = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("message-1", "answer"),
            NOW.into(),
        )
        .await
        .unwrap();

    assert!(first.appended.is_empty());
    assert!(second.appended.is_empty());
    assert_eq!(assembly.content, "first answer");
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(working.clone())
    );

    let boundary = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("message-2", "second answer"),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        boundary.appended.as_slice(),
        [AgentLogEntry {
            kind: AgentLogKind::AgentMessage { content },
            ..
        }] if content == "first answer"
    ));
    assert_eq!(assembly.content, "second answer");
    assert!(assembly.prompt_evidence);

    let terminal = recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert!(terminal.turn_closed);
    assert!(assembly.is_empty());
    assert!(matches!(
        terminal.appended.as_slice(),
        [
            AgentLogEntry {
                kind: AgentLogKind::AgentMessage { content },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::TurnCompleted { .. },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::StatusChanged {
                    to: AgentStatus::Ready,
                    ..
                },
                ..
            }
        ] if content == "second answer"
    ));
    let stored = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
}

#[tokio::test]
async fn first_message_progresses_from_write_confirmation_to_completion() {
    let (store, agent) = ready_agent(Some("Inspect the proof.")).await;
    let mutation = begin_first_message(&store, agent);
    let working = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();

    let observed = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "Done."),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        observed.appended.as_slice(),
        [AgentLogEntry {
            kind: AgentLogKind::FirstMessageChanged {
                state: FirstMessageDelivery::ExecutionObserved,
                ..
            },
            ..
        }]
    ));
    let observed_agent = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_eq!(
        observed_agent.conversation.first_message.unwrap().state,
        FirstMessageDelivery::ExecutionObserved
    );

    recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    let completed = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_eq!(
        completed.conversation.first_message.unwrap().state,
        FirstMessageDelivery::Completed
    );
    assert_eq!(completed.status, AgentStatus::Ready);
}

#[tokio::test]
async fn recorder_rejects_stale_generations_without_touching_live_assembly() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();

    assert_eq!(
        recorder
            .record_update(
                &working.id,
                2,
                &mut assembly,
                message_chunk("reply", "ignored"),
                NOW.into(),
            )
            .await,
        Err(AgentConversationError::StaleGeneration)
    );
    assert!(assembly.is_empty());
    assert_eq!(store.get_agent(&working.id).await.unwrap(), Some(working));
}

#[tokio::test]
async fn recorder_rejects_updates_after_stop_or_delete_without_mutation() {
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        for update in [
            ConversationUpdate::Lost { frames: 2 },
            ConversationUpdate::SignInRequired {
                url: "https://signin.example/device".into(),
                code: Some("ABCD".into()),
                message: Some("Sign in".into()),
            },
        ] {
            let (store, mut terminal) = ready_agent(None).await;
            terminal.status = status;
            terminal.stop_reason = Some(AgentStopReason::Owner);
            terminal.stopped_at = Some(NOW.into());
            terminal.deleted_at = (status == AgentStatus::Deleted).then(|| NOW.into());
            terminal.updated_at = NOW.into();
            let expected_revision = terminal.revision;
            terminal.revision += 1;
            store
                .save_agent(expected_revision, terminal.clone())
                .await
                .unwrap();
            let logs_before = store.agent_log(&terminal.id, 0, 200).await.unwrap();
            let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
            let mut assembly = AgentMessageAssembly::default();

            assert_eq!(
                recorder
                    .record_update(&terminal.id, 1, &mut assembly, update, NOW.into(),)
                    .await,
                Err(AgentConversationError::InvalidState(
                    "conversation updates require a live agent"
                ))
            );
            assert!(assembly.is_empty());
            assert_eq!(store.get_agent(&terminal.id).await.unwrap(), Some(terminal));
            assert_eq!(
                store
                    .agent_log(&logs_before[0].agent_id, 0, 200)
                    .await
                    .unwrap(),
                logs_before
            );
        }
    }
}

#[tokio::test]
async fn approval_requests_wait_and_can_be_reconstructed_after_restart() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    let request = PermissionRequest {
        id: "approval-1".into(),
        locator: Some(PermissionLocator::new("runtime-route-1")),
        request: RequestPermissionRequest::new(
            "conversation-1",
            ToolCallUpdate::new("call-1", ToolCallUpdateFields::new().title("Run checks")),
            vec![PermissionOption::new(
                "allow-once",
                "Allow once",
                PermissionOptionKind::AllowOnce,
            )],
        ),
    };

    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Asked(request),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::WaitingApproval
    );

    drop(recorder);
    let restarted_store = store.clone();
    let pending = restarted_store
        .pending_agent_approvals(&working.id)
        .await
        .unwrap();
    assert!(matches!(
        pending.as_slice(),
        [AgentLogEntry {
            kind: AgentLogKind::ApprovalRequested { approval_id, .. },
            ..
        }] if approval_id == "approval-1"
    ));
}

#[tokio::test]
async fn concurrent_approval_requests_preserve_the_recorder_and_each_pending_identity() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    for id in ["approval-1", "approval-2"] {
        let request = PermissionRequest {
            id: id.into(),
            locator: Some(PermissionLocator::new(format!("route-{id}"))),
            request: RequestPermissionRequest::new(
                "conversation-1",
                ToolCallUpdate::new(id, ToolCallUpdateFields::new().title("Read guidance")),
                vec![PermissionOption::new(
                    "allow-once",
                    "Allow once",
                    PermissionOptionKind::AllowOnce,
                )],
            ),
        };
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                ConversationUpdate::Asked(request),
                NOW.into(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::WaitingApproval
    );
    let pending = store.pending_agent_approvals(&working.id).await.unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(
        store
            .agent_log(&working.id, 0, 200)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::StatusChanged {
                    to: AgentStatus::WaitingApproval,
                    ..
                }
            ))
            .count(),
        1
    );
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "Both requests are visible."),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(assembly.content, "Both requests are visible.");
}

#[tokio::test]
async fn failed_turn_returns_to_ready() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();

    let outcome = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Failed {
                message: "runtime refused the turn".into(),
                capability: Some("turn_refused".into()),
            },
            NOW.into(),
        )
        .await
        .unwrap();

    assert!(outcome.turn_closed);
    let stored = store.get_agent(&working.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
}

#[tokio::test]
async fn failed_atomic_commits_preserve_the_aggregate_and_live_assembly() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "still live"),
            NOW.into(),
        )
        .await
        .unwrap();
    let assembly_before = assembly.clone();
    let aggregate_before = store.get_agent(&working.id).await.unwrap().unwrap();
    {
        let mut state = store.lock();
        state
            .agent_logs
            .get_mut(&working.id)
            .unwrap()
            .push(AgentLogEntry {
                agent_id: working.id.clone(),
                seq: aggregate_before.conversation.last_seq + 1,
                appended_at: NOW.into(),
                kind: AgentLogKind::ConversationGap { frames: 1 },
            });
    }

    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                ConversationUpdate::Lost { frames: 2 },
                NOW.into(),
            )
            .await,
        Err(AgentConversationError::Corrupt(_))
    ));
    assert_eq!(assembly, assembly_before);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(aggregate_before)
    );
    assert_eq!(
        store.agent_log(&working.id, 0, 200).await.unwrap().len(),
        usize::try_from(working.conversation.last_seq + 1).unwrap()
    );
}

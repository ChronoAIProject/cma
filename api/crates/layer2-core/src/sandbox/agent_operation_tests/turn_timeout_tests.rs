use super::command_work_tests::{admit, fixture, request};
use super::fixtures::*;
use crate::sandbox::{agent_io::*, store::*, turn_timeout::*, *};
use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, SessionUpdate};

async fn armed() -> (
    std::sync::Arc<InMemorySandboxStore>,
    Agent,
    AdmittedAgentCommand,
    String,
) {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let first = admit(
        &store,
        &agent,
        &parent,
        "timeout-one",
        AgentCommand::message("first"),
    )
    .await;
    let work = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    let command = store
        .reconcile_command(&first.receipt.envelope.command_id)
        .await
        .unwrap();
    let mut current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let dispatch = command.work.dispatch.as_ref().unwrap();
    let now = crate::now_rfc3339();
    let now_ms = crate::seconds_from_rfc3339(&now).unwrap() as i64 * 1_000;
    current.conversation.turn_timeout = Some(
        TurnTimeout::admit(
            TurnTimeoutBinding {
                agent_id: current.id.clone(),
                creation_key: current.creation_key.clone(),
                creation_fingerprint: current.creation_fingerprint.clone(),
                command_id: command.receipt.envelope.command_id.clone(),
                turn_sequence: dispatch.turn_sequence,
                operation_sequence: dispatch.operation_sequence,
                runtime_generation: dispatch.runtime_generation,
                runtime_boot: "b".repeat(32),
                conversation_id: current.conversation.conversation_id.clone().unwrap(),
                prompt_id: layer3_adapter::PromptIdentity::new(
                    current.conversation.conversation_id.as_ref().unwrap(),
                    dispatch.runtime_generation,
                    dispatch.turn_sequence,
                )
                .as_str()
                .into(),
            },
            uuid::Uuid::new_v4().to_string(),
            TurnTimeoutCause::ServiceDeadline,
            now_ms - 1_000,
            now_ms,
        )
        .unwrap(),
    );
    current.revision += 1;
    store.seed_turn_policy_fixture(current.clone());
    (store, current, command, now)
}

#[tokio::test]
async fn turn_timeout_preserves_partial_output_settles_command_and_blocks_next_input() {
    let (store, agent, command, now) = armed().await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    recorder
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::RuntimeOutput {
                receipt: layer3_adapter::RuntimeOutputReceipt::new(
                    layer3_adapter::RuntimeOutputSource {
                        stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                        cursor: 1,
                        native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        payload_sha256: "a".repeat(64),
                    },
                    tokio::sync::mpsc::channel(1).0,
                ),
                update: Some(layer3_adapter::RuntimeOutputUpdate::new(
                    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        ContentBlock::from("partial answer"),
                    ))),
                    false,
                )),
            },
            now.clone(),
        )
        .await
        .unwrap();
    let snapshot = store
        .agent_message_snapshot(&agent.id)
        .await
        .unwrap()
        .unwrap();
    let previous_output = snapshot.checkpoint.as_ref().unwrap().output.clone();
    assert_eq!(previous_output.as_ref().unwrap().cursor, 1);
    let proof = TurnTimeoutSettlement::prepare(snapshot, command.clone(), 1, now.clone()).unwrap();
    assert_eq!(
        store
            .commit_agent_conversation(proof.mutation().clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .commit_agent_message_update(
                proof.snapshot().agent.clone(),
                proof.snapshot().checkpoint.clone(),
                proof.checkpoint().clone(),
                Some(proof.mutation().clone()),
            )
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let (a, b) = tokio::join!(
        store.commit_turn_timeout(&proof),
        store.commit_turn_timeout(&proof)
    );
    a.unwrap();
    b.unwrap();
    let terminal = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(terminal.conversation.active_turn, None);
    assert!(terminal
        .conversation
        .turn_timeout
        .as_ref()
        .unwrap()
        .blocks_dispatch());
    let failed = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(
        failed.work.observation.as_ref().unwrap().delivery,
        command.work.observation.as_ref().unwrap().delivery
    );
    let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|row| matches!(&row.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    assert!(history.iter().any(|row| matches!(&row.kind, AgentLogKind::AgentMessage { content, .. } if content == "partial answer")));
    assert_eq!(
        store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .output,
        previous_output
    );
    let mut second = command.clone();
    second.status = AgentResponseStatus::Queued;
    second.work = Default::default();
    assert!(!command_ready_for_claim(&second, Some(&terminal), &now));
    let rejected = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), recording_sender(Ok(())).0),
    )
    .send_message(&owner(), &agent.id, "second".into())
    .await;
    assert!(rejected.is_err());
    let old_prompt = layer3_adapter::PromptIdentity::new(
        agent.conversation.conversation_id.as_ref().unwrap(),
        3,
        agent.conversation.active_turn.unwrap(),
    );
    recorder
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::FailedForPrompt {
                prompt_id: old_prompt,
                message: "late failure".into(),
                capability: None,
            },
            now,
        )
        .await
        .unwrap();
    store.commit_turn_timeout(&proof).await.unwrap();
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), history);
}

#[tokio::test]
async fn turn_timeout_loses_to_a_real_result_or_stop_intent() {
    for stop in [false, true] {
        let (store, agent, command, now) = armed().await;
        let snapshot = store
            .agent_message_snapshot(&agent.id)
            .await
            .unwrap()
            .unwrap();
        let proof = TurnTimeoutSettlement::prepare(snapshot, command, 1, now.clone()).unwrap();
        if stop {
            let mut changed = agent.clone();
            changed.request_stop(AgentStopReason::Owner, now).unwrap();
            store.save_agent(agent.revision, changed).await.unwrap();
        } else {
            DurableAgentUpdateRecorder::new(store.clone())
                .record_update(
                    &agent.id,
                    3,
                    &mut AgentMessageAssembly::default(),
                    ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                        layer3_adapter::TurnStopReason::EndTurn,
                    )),
                    now,
                )
                .await
                .unwrap();
        }
        assert!(store.commit_turn_timeout(&proof).await.is_err());
        let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert!(!history.iter().any(|row| matches!(&row.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_turn_timeout")));
    }
}

#[tokio::test]
async fn ordinary_saves_cannot_drop_replace_or_introduce_a_turn_policy() {
    let (store, agent, command, now) = armed().await;
    let snapshot = store
        .agent_message_snapshot(&agent.id)
        .await
        .unwrap()
        .unwrap();
    let proof = TurnTimeoutSettlement::prepare(snapshot, command, 1, now).unwrap();
    for current in [agent.clone(), proof.mutation().agent.clone()] {
        store.seed_turn_policy_fixture(current.clone());
        let mut erased = current.clone();
        erased.revision += 1;
        erased.conversation.turn_timeout = None;
        assert_eq!(
            store.save_agent(current.revision, erased.clone()).await,
            Err(SandboxStoreError::Conflict)
        );
        let mut replaced = erased.clone();
        replaced.conversation.turn_timeout = Some(
            current
                .conversation
                .turn_timeout
                .as_ref()
                .unwrap()
                .supersede(1)
                .unwrap_or_else(|_| agent.conversation.turn_timeout.clone().unwrap()),
        );
        assert_eq!(
            store.save_agent(current.revision, replaced).await,
            Err(SandboxStoreError::Conflict)
        );
        store.seed_turn_policy_fixture(erased.clone());
        let mut introduced = erased.clone();
        introduced.revision += 1;
        introduced.conversation.turn_timeout = current.conversation.turn_timeout;
        assert_eq!(
            store.save_agent(erased.revision, introduced).await,
            Err(SandboxStoreError::Conflict)
        );
    }
}

#[tokio::test]
async fn command_status_uses_the_same_external_wait_without_native_approval_actions() {
    use crate::sandbox::turn_timeout::approval::*;
    let (store, mut agent, command, now) = armed().await;
    let now_ms = crate::seconds_from_rfc3339(&now).unwrap() as i64 * 1_000;
    let previous = agent.conversation.turn_timeout.take().unwrap();
    let admitted = TurnTimeout::admit_approval(
        previous.binding().clone(),
        uuid::Uuid::new_v4().to_string(),
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: now_ms + 901_000,
        },
        now_ms,
    )
    .unwrap();
    let operation = ApprovalOperationBinding {
        operation_id: "external-operation".into(),
        model_attempt_id: "model-attempt".into(),
        provider: "external".into(),
        authority_digest: "c".repeat(64),
    };
    let registered = admitted
        .register_approval_operation(1, operation.clone(), now_ms)
        .unwrap();
    let pending = registered
        .observe_pending_approval(
            registered.revision(),
            &PendingApprovalReceipt {
                binding: operation.clone(),
                review_id: "private-review".into(),
                revision: 1,
                created_at_ms: now_ms,
                expires_at_ms: now_ms + 901_000,
            },
            now_ms,
        )
        .unwrap();
    agent.conversation.turn_timeout = Some(pending.clone());
    store.seed_turn_policy_fixture(agent.clone());
    let waiting = store
        .reconcile_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentResponseStatus::WaitingAction);
    assert!(store
        .pending_agent_approvals(&agent.id)
        .await
        .unwrap()
        .is_empty());
    let approved = pending
        .observe_approval_resolution(
            pending.revision(),
            &ResolvedApprovalReceipt {
                binding: operation,
                revision: 2,
                resolved_at_ms: now_ms,
                resolution: ApprovalResolution::Approved,
            },
            now_ms,
        )
        .unwrap();
    agent.conversation.turn_timeout = Some(approved);
    store.seed_turn_policy_fixture(agent);
    assert_eq!(
        store
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .status,
        AgentResponseStatus::InProgress
    );
}

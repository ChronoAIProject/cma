use super::*;
use crate::sandbox::store::AgentConnectionRecoveryCandidate;

async fn publish_legacy_connection(
    store: &super::super::super::InMemorySandboxStore,
    id: &AgentId,
) {
    let previous = store.agent_message_checkpoint(id).await.unwrap().unwrap();
    publish_connection(store, id).await;
    // Reproduce sidecars left by the already-deployed writer, without changing
    // its append-only log, native identity, or admitted turn.
    store
        .lock()
        .agent_message_checkpoints
        .insert(id.clone(), previous);
}

pub(super) async fn publish_connection(
    store: &super::super::super::InMemorySandboxStore,
    id: &AgentId,
) {
    let current = store.get_agent(id).await.unwrap().unwrap();
    store
        .schedule_agent_connection_recovery(
            AgentConnectionRecoveryCandidate::from_agent(&current)
                .unwrap()
                .restart_request(NOW.into()),
        )
        .await
        .unwrap();
    let claimed = store
        .claim_agent_work(id, "reconnector", NOW.into(), "2999-01-01T00:00:00Z".into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("reconnector", fence, AgentWorkStep::None, NOW.into())
        .unwrap();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = NOW.into();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "reconnector".into(),
            fence,
            agent: next,
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            }],
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn attachment_restores_without_native_updates_and_later_publication_keeps_anchor() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "retained prefix"),
            NOW.into(),
        )
        .await
        .unwrap();
    let original = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    publish_legacy_connection(&store, &working.id).await;
    let current = store.get_agent(&working.id).await.unwrap().unwrap();
    let before_logs = store.agent_log(&working.id, 0, 200).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    replacement
        .restore_assembly(&working.id, 1, &mut assembly)
        .await
        .unwrap();
    assert_eq!(assembly.content, "retained prefix");
    assert_eq!(
        store.agent_message_checkpoint(&working.id).await.unwrap(),
        original.reanchored(&current)
    );
    assert_eq!(store.get_agent(&working.id).await.unwrap(), Some(current));
    assert_eq!(
        store.agent_log(&working.id, 0, 200).await.unwrap(),
        before_logs
    );
    // Connection completion can race after restore, before any native update.
    publish_connection(&store, &working.id).await;
    let published = store.get_agent(&working.id).await.unwrap().unwrap();
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&published));
    replacement
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["retained prefix"]
    );
}

#[tokio::test]
async fn replacement_recovers_prefix_across_repeated_connection_publications() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "retained prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    let checkpoint = store.agent_message_checkpoint(&working.id).await.unwrap();
    for _ in 0..9 {
        publish_legacy_connection(&store, &working.id).await;
    }
    assert_eq!(
        store.agent_message_checkpoint(&working.id).await.unwrap(),
        checkpoint
    );
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("reply", "suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(fresh.content, "retained prefix suffix");
    let current = store.get_agent(&working.id).await.unwrap().unwrap();
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&current));
    let terminal = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let outcome = terminal
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            turn_ended(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["retained prefix suffix"]
    );
}

#[tokio::test]
async fn quiet_reattachment_durably_repairs_anchor_before_another_semantic_commit() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "retained prefix"),
            NOW.into(),
        )
        .await
        .unwrap();
    publish_legacy_connection(&store, &working.id).await;
    let current = store.get_agent(&working.id).await.unwrap().unwrap();
    let logs = store.agent_log(&working.id, 0, 200).await.unwrap();
    let quiet = ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
        ContentBlock::from("ephemeral reconnect thought"),
    )));
    let outcome = recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            quiet,
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(outcome.appended.is_empty());
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(store.agent_log(&working.id, 0, 200).await.unwrap(), logs);
    let repaired = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert!(repaired.matches(&current));
    assert_eq!(repaired.assembly.content, "retained prefix");
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            reconciliation_fence: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: 1,
            agent: next.clone(),
            appended_at: NOW.into(),
            agent_logs: vec![AgentLogKind::ConversationGap { frames: 1 }],
        })
        .await
        .unwrap();
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&next));
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            turn_ended(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["retained prefix"]
    );
}

#[tokio::test]
async fn publication_after_checkpoint_read_retries_the_same_suffix_once() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    let interleaved = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(false),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(true),
    });
    let replacement = DurableAgentUpdateRecorder::new(interleaved);
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("reply", "suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(&working.id, 1, &mut fresh, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["prefix suffix"]
    );
}

#[tokio::test]
async fn lost_receipt_replay_after_publication_restores_prefix_without_duplicate_update() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(true),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(false),
    });
    let recorder = DurableAgentUpdateRecorder::new(flaky.clone());
    let mut assembly = AgentMessageAssembly::default();
    let chunk = message_chunk("reply", "retained answer");
    assert!(matches!(
        recorder
            .record_update(&working.id, 1, &mut assembly, chunk.clone(), NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let original = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    publish_legacy_connection(&store, &working.id).await;
    let current = store.get_agent(&working.id).await.unwrap().unwrap();
    flaky.fail_next.store(true, Ordering::SeqCst);
    assert!(matches!(
        recorder
            .record_update(&working.id, 1, &mut assembly, chunk.clone(), NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let repaired = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repaired, original.reanchored(&current).unwrap());
    let replay = recorder
        .record_update(&working.id, 1, &mut assembly, chunk, NOW.into())
        .await
        .unwrap();
    assert_eq!(replay, original.outcome);
    assert_eq!(assembly.content, "retained answer");
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            reconciliation_fence: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: 1,
            agent: next.clone(),
            appended_at: NOW.into(),
            agent_logs: vec![AgentLogKind::ConversationGap { frames: 1 }],
        })
        .await
        .unwrap();
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap()
        .matches(&next));
    recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["retained answer"]
    );
}

#[tokio::test]
async fn recovery_rejects_semantic_foreign_gapped_or_excessive_log_suffix_without_writes() {
    for corruption in 0..7 {
        let (store, working, recorder) = working_agent().await;
        recorder
            .record_update(
                &working.id,
                1,
                &mut AgentMessageAssembly::default(),
                message_chunk("reply", "prefix"),
                NOW.into(),
            )
            .await
            .unwrap();
        publish_legacy_connection(&store, &working.id).await;
        {
            let mut state = store.lock();
            let entry = state
                .agent_logs
                .get_mut(&working.id)
                .unwrap()
                .last_mut()
                .unwrap();
            match corruption {
                0 => {
                    entry.kind = AgentLogKind::AgentMessage {
                        content: "already committed".into(),
                    }
                }
                1 => {
                    entry.kind = AgentLogKind::TurnStarted {
                        model: None,
                        turn_sequence: entry.seq,
                        origin_key: None,
                    }
                }
                2 => {
                    entry.kind = AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::PublishReady,
                    }
                }
                3 => entry.agent_id = AgentId("agt_foreign".into()),
                4 => entry.seq += 1,
                5 => {
                    state.agent_logs.get_mut(&working.id).unwrap().pop();
                }
                _ => {
                    state
                        .agents
                        .get_mut(&working.id)
                        .unwrap()
                        .conversation
                        .last_seq += 200
                }
            }
        }
        let before = store.lock().clone();
        let mut assembly = AgentMessageAssembly::default();
        assert!(
            matches!(
                recorder
                    .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
                    .await,
                Err(AgentConversationError::Corrupt(_))
                    | Err(AgentConversationError::Unavailable(_))
            ),
            "case {corruption}"
        );
        assert!(assembly.is_empty());
        let after = store.lock();
        assert_eq!(
            after.agents.get(&working.id),
            before.agents.get(&working.id)
        );
        assert_eq!(
            after.agent_logs.get(&working.id),
            before.agent_logs.get(&working.id)
        );
        assert_eq!(
            after.agent_message_checkpoints.get(&working.id),
            before.agent_message_checkpoints.get(&working.id)
        );
    }
}

use super::*;
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};

const STREAM: &str = "d99003d1-0c3c-46cd-aa47-9bbdbed50b12";

fn retained(cursor: u64, update: Option<ConversationUpdate>) -> ConversationUpdate {
    let (ack, _) = tokio::sync::mpsc::channel(1);
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: STREAM.into(),
                cursor,
                native_conversation_id: "conversation-1".into(),
                payload_sha256: format!("{cursor:064x}"),
            },
            ack,
        ),
        update: update.map(Box::new),
    }
}

#[tokio::test]
async fn idle_resume_history_advances_output_cursor_without_reopening_a_completed_turn() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "saved answer"),
            NOW.into(),
        )
        .await
        .unwrap();
    recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    let ready = store.get_agent(&working.id).await.unwrap().unwrap();
    let history = store.agent_log(&working.id, 0, 200).await.unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);

    // Loading the retained conversation emits its history on a new runtime
    // output stream. These are new receipts, but carry no current prompt.
    let replay = [
        message_chunk("reply", "saved answer").without_prompt_evidence(),
        ConversationUpdate::Said(SessionUpdate::ToolCall(layer3_adapter::ToolCall::new(
            "old-call",
            "Earlier work",
        )))
        .without_prompt_evidence(),
        turn_ended().without_prompt_evidence(),
    ];
    for (index, update) in replay.into_iter().enumerate() {
        let outcome = recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                retained(index as u64 + 1, Some(update)),
                NOW.into(),
            )
            .await
            .expect("idle history must not break the restored connection");
        assert!(outcome.appended.is_empty());
        assert!(!outcome.turn_closed);
    }
    assert!(assembly.is_empty());
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(ready.clone())
    );
    assert_eq!(store.agent_log(&working.id, 0, 200).await.unwrap(), history);
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 3);

    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                message_chunk("unexpected", "no admitted turn"),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::InvalidState("no turn is active"))
    ));

    // A new recorder can acknowledge the same output again, then accept a real
    // next turn without duplicating the loaded history or losing new output.
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(&working.id, 1, &mut fresh, retained(3, None), NOW.into())
        .await
        .unwrap();
    store
        .commit_agent_conversation(begin_turn(&store, ready))
        .await
        .unwrap();
    for update in [
        retained(4, Some(message_chunk("next", "new answer"))),
        retained(5, Some(turn_ended())),
    ] {
        replacement
            .record_update(&working.id, 1, &mut fresh, update, NOW.into())
            .await
            .unwrap();
    }
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["saved answer", "new answer"]
    );
}

#[tokio::test]
async fn recovered_active_turn_keeps_output_without_new_prompt_evidence() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        retained(
            1,
            Some(message_chunk("reply", "recovered answer").without_prompt_evidence()),
        ),
        retained(2, Some(turn_ended().without_prompt_evidence())),
    ] {
        recorder
            .record_update(&working.id, 1, &mut assembly, update, NOW.into())
            .await
            .unwrap();
    }
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["recovered answer"]
    );
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
}

#[tokio::test]
async fn idle_resume_keeps_conversation_metadata() {
    let (store, ready) = ready_agent(None).await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let update = ConversationUpdate::Said(
        serde_json::from_value(serde_json::json!({
            "sessionUpdate": "current_mode_update", "currentModeId": "read-only"
        }))
        .unwrap(),
    )
    .without_prompt_evidence();
    let outcome = recorder
        .record_update(
            &ready.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, Some(update)),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome.appended.as_slice(), [AgentLogEntry { kind: AgentLogKind::ModeChanged { mode }, .. }] if mode == "read-only")
    );
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
}

#[tokio::test]
async fn lost_ack_replays_prefix_and_terminal_once_across_replacement_and_next_turn() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        retained(1, None),
        retained(2, Some(message_chunk("reply", "retained answer"))),
        retained(3, Some(turn_ended())),
    ] {
        recorder
            .record_update(&working.id, 1, &mut assembly, update, NOW.into())
            .await
            .unwrap();
    }
    let before = store.agent_log(&working.id, 0, 200).await.unwrap();
    assert_eq!(messages(&before), ["retained answer"]);
    let ready = store.get_agent(&working.id).await.unwrap().unwrap();
    store
        .commit_agent_conversation(begin_turn(&store, ready))
        .await
        .unwrap();
    let next_turn = store.get_agent(&working.id).await.unwrap().unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    for update in [
        retained(1, None),
        retained(2, Some(message_chunk("reply", "retained answer"))),
        retained(3, Some(turn_ended())),
    ] {
        let outcome = replacement
            .record_update(&working.id, 1, &mut fresh, update, NOW.into())
            .await
            .unwrap();
        assert!(outcome.appended.is_empty());
        assert!(!outcome.turn_closed);
    }
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(next_turn.clone())
    );
    assert!(fresh.is_empty());
    // A local/legacy update must not erase the reliable cursor.
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            message_chunk("next", "new "),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(&working.id, 1, &mut fresh, retained(3, None), NOW.into())
        .await
        .unwrap();
    assert_eq!(fresh.content, "new ");
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            retained(4, Some(message_chunk("next", "answer"))),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            retained(5, Some(turn_ended())),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["retained answer", "new answer"]
    );
}

#[tokio::test]
async fn committed_output_survives_lost_store_reply_and_lost_ack_with_fresh_assembly() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(true),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(false),
    });
    let recorder = DurableAgentUpdateRecorder::new(flaky.clone());
    let update = retained(1, Some(message_chunk("reply", "exactly once")));
    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut AgentMessageAssembly::default(),
                update.clone(),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(&working.id, 1, &mut fresh, update, NOW.into())
        .await
        .unwrap();
    assert_eq!(fresh.content, "exactly once");
    flaky.fail_next.store(true, Ordering::SeqCst);
    assert!(matches!(
        recorder
            .record_update(
                &working.id,
                1,
                &mut fresh,
                retained(2, Some(turn_ended())),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    // A terminal from a completed old prompt may parse as a no-op on a new adapter.
    replacement
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(2, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["exactly once"]
    );
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
}

#[tokio::test]
async fn protocol_only_output_is_checkpointed_without_public_revision_or_log_change() {
    let (store, working, recorder) = working_agent().await;
    let logs = store.agent_log(&working.id, 0, 200).await.unwrap();
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, None),
            NOW.into(),
        )
        .await
        .unwrap();
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 1);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(working.clone())
    );
    assert_eq!(store.agent_log(&working.id, 0, 200).await.unwrap(), logs);
}

#[tokio::test]
async fn foreign_or_skipped_output_cannot_mutate_or_advance_the_receipt() {
    let (store, working, recorder) = working_agent().await;
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, None),
            NOW.into(),
        )
        .await
        .unwrap();
    let previous = store.agent_message_checkpoint(&working.id).await.unwrap();
    for case in 0..5 {
        let mut update = retained(2, Some(message_chunk("reply", "refused")));
        let ConversationUpdate::RuntimeOutput { receipt, .. } = &mut update else {
            unreachable!()
        };
        match case {
            0 => receipt.source.native_conversation_id = "another-session".into(),
            1 => receipt.source.stream_id = uuid::Uuid::new_v4().to_string(),
            2 => receipt.source.cursor = 3,
            3 => receipt.source.cursor = 1, // Same cursor with a different immutable payload hash.
            _ => receipt.source.payload_sha256 = "invalid".into(),
        }
        assert!(matches!(
            recorder
                .record_update(
                    &working.id,
                    1,
                    &mut AgentMessageAssembly::default(),
                    update,
                    NOW.into()
                )
                .await,
            Err(AgentConversationError::Corrupt(_))
        ));
        assert_eq!(
            store.agent_message_checkpoint(&working.id).await.unwrap(),
            previous
        );
        assert_eq!(
            store.get_agent(&working.id).await.unwrap(),
            Some(working.clone())
        );
    }
}

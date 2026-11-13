use super::*;
use crate::sandbox::{AgentOutputCoalescePolicy, AgentOutputCommit};
use std::time::Duration;

fn recorder<S: crate::sandbox::store::AgentConversationUpdateStore + 'static>(
    store: Arc<S>,
    frames: usize,
) -> Arc<DurableAgentUpdateRecorder> {
    DurableAgentUpdateRecorder::with_output_policy(
        store,
        Arc::default(),
        None,
        false,
        false,
        Some(AgentOutputCoalescePolicy::new(Duration::from_millis(250), frames).unwrap()),
    )
}

async fn record(
    recorder: &dyn AgentUpdateRecorder,
    id: &AgentId,
    assembly: &mut AgentMessageAssembly,
    cursor: u64,
    update: Option<ConversationUpdate>,
) -> AgentOutputCommit {
    recorder
        .record_output(id, 1, assembly, retained(cursor, update), NOW.into())
        .await
        .unwrap()
}

#[tokio::test]
async fn default_off_commits_each_receipt_and_count_limit_commits_only_last_cursor() {
    let (store, working, disabled) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    for cursor in 1..=4 {
        assert_eq!(
            record(disabled.as_ref(), &working.id, &mut assembly, cursor, None)
                .await
                .committed
                .unwrap()
                .cursor,
            cursor
        );
    }
    let before = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    let enabled = recorder(Arc::new(store.clone()), 3);
    for cursor in 5..=7 {
        let result = record(enabled.as_ref(), &working.id, &mut assembly, cursor, None).await;
        let persisted = store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap();
        if cursor < 7 {
            assert_eq!(result.committed, None);
            assert_eq!(persisted, before);
        } else {
            assert_eq!(result.committed.unwrap().cursor, 7);
            assert_eq!(persisted.output.unwrap().cursor, 7);
            assert!(assembly.buffered_output().is_none());
            assert!(persisted.assembly.output_state.is_none());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn reasoning_is_live_while_quiet_receipts_wait_for_timer_or_text_commit() {
    let (store, working, _) = working_agent().await;
    let enabled = recorder(Arc::new(store.clone()), 64);
    let mut live = enabled.subscribe_live(&working.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    let thought = ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
        ContentBlock::from("reasoning"),
    )));
    assert!(record(
        enabled.as_ref(),
        &working.id,
        &mut assembly,
        1,
        Some(thought)
    )
    .await
    .committed
    .is_none());
    assert_eq!(live.try_recv().unwrap().delta, "reasoning");
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .is_none());
    let deadline = assembly.output_flush_deadline().unwrap();
    tokio::time::advance(Duration::from_millis(249)).await;
    assert!(tokio::time::Instant::now() < deadline);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        enabled
            .flush_output(&working.id, 1, &mut assembly, NOW.into())
            .await
            .unwrap()
            .unwrap()
            .cursor,
        1
    );
    assert!(assembly.output_flush_deadline().is_none());
    assert!(
        record(enabled.as_ref(), &working.id, &mut assembly, 2, None)
            .await
            .committed
            .is_none()
    );
    assert_eq!(
        record(
            enabled.as_ref(),
            &working.id,
            &mut assembly,
            3,
            Some(message_chunk("reply", "answer"))
        )
        .await
        .committed
        .unwrap()
        .cursor,
        3
    );
    let saved = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.assembly.content, "answer");
    assert_eq!(saved.output.unwrap().cursor, 3);
    assert_eq!(live.try_recv().unwrap().delta, "answer");
    // An idle gap does not restart the durable-cursor clock on a new quiet frame.
    tokio::time::advance(Duration::from_millis(251)).await;
    assert_eq!(
        record(enabled.as_ref(), &working.id, &mut assembly, 4, None)
            .await
            .committed
            .unwrap()
            .cursor,
        4
    );
}

#[tokio::test]
async fn buffered_duplicates_never_ack_and_invalid_positions_are_refused() {
    let (store, working, _) = working_agent().await;
    let enabled = recorder(Arc::new(store.clone()), 64);
    let mut assembly = AgentMessageAssembly::default();
    for cursor in 1..=3 {
        assert!(
            record(enabled.as_ref(), &working.id, &mut assembly, cursor, None)
                .await
                .committed
                .is_none()
        );
    }
    for cursor in [1, 2, 3] {
        assert!(
            record(enabled.as_ref(), &working.id, &mut assembly, cursor, None)
                .await
                .committed
                .is_none()
        );
    }
    for invalid in ["hash", "stream", "gap", "conversation", "zero"] {
        let mut update = retained(if invalid == "hash" { 2 } else { 4 }, None);
        let ConversationUpdate::RuntimeOutput { receipt, .. } = &mut update else {
            unreachable!()
        };
        match invalid {
            "hash" => receipt.source.payload_sha256 = "f".repeat(64),
            "stream" => receipt.source.stream_id = uuid::Uuid::new_v4().to_string(),
            "gap" => receipt.source.cursor = 5,
            "conversation" => receipt.source.native_conversation_id = "other".into(),
            "zero" => receipt.source.cursor = 0,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                enabled
                    .record_output(&working.id, 1, &mut assembly, update, NOW.into())
                    .await,
                Err(AgentConversationError::Corrupt(_))
            ),
            "{invalid}"
        );
        assert_eq!(assembly.buffered_output().unwrap().cursor, 3);
    }
    assert!(store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn older_durable_replay_preserves_newer_buffer_and_generation_is_fenced() {
    let (store, working, disabled) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    record(disabled.as_ref(), &working.id, &mut assembly, 1, None).await;
    let enabled = recorder(Arc::new(store.clone()), 64);
    record(enabled.as_ref(), &working.id, &mut assembly, 2, None).await;
    let deadline = assembly.output_flush_deadline();
    assert_eq!(
        record(enabled.as_ref(), &working.id, &mut assembly, 1, None)
            .await
            .committed
            .unwrap()
            .cursor,
        1
    );
    assert_eq!(assembly.buffered_output().unwrap().cursor, 2);
    assert_eq!(assembly.output_flush_deadline(), deadline);
    assert_eq!(
        enabled
            .record_output(&working.id, 2, &mut assembly, retained(3, None), NOW.into())
            .await,
        Err(AgentConversationError::StaleGeneration)
    );
    assert_eq!(
        enabled
            .flush_output(&working.id, 1, &mut assembly, NOW.into())
            .await
            .unwrap()
            .unwrap()
            .cursor,
        2
    );
}

#[tokio::test]
async fn uncertain_flush_keeps_identity_and_blocks_another_input_until_reconciled() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(true),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(false),
    });
    let enabled = recorder(flaky, 64);
    let mut assembly = AgentMessageAssembly::default();
    for cursor in 1..=3 {
        record(enabled.as_ref(), &working.id, &mut assembly, cursor, None).await;
    }
    assert!(matches!(
        enabled
            .flush_output(&working.id, 1, &mut assembly, NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let pending = assembly.pending_update.clone().unwrap();
    let saved = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.update_id, pending.id);
    assert!(matches!(
        enabled
            .record_output(&working.id, 1, &mut assembly, retained(4, None), NOW.into())
            .await,
        Err(AgentConversationError::InvalidState(_))
    ));
    assert_eq!(assembly.pending_update.as_ref().unwrap().id, pending.id);
    assert_eq!(
        enabled
            .flush_output(&working.id, 1, &mut assembly, NOW.into())
            .await
            .unwrap()
            .unwrap()
            .cursor,
        3
    );
    assert_eq!(
        store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap(),
        saved
    );
    assert!(assembly.pending_update.is_none());
    assert!(assembly.buffered_output().is_none());
}

#[tokio::test]
async fn conflict_retries_preserve_batch_and_identity_terminal_flushes() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(false),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(true),
    });
    let enabled = recorder(flaky, 64);
    let mut assembly = AgentMessageAssembly::default();
    record(enabled.as_ref(), &working.id, &mut assembly, 1, None).await;
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: layer3_adapter::PromptIdentity::new(
            "conversation-1",
            1,
            working.conversation.active_turn.unwrap(),
        ),
        response: layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
    };
    let result = record(
        enabled.as_ref(),
        &working.id,
        &mut assembly,
        2,
        Some(terminal),
    )
    .await;
    assert!(result.outcome.turn_closed);
    assert_eq!(result.committed.unwrap().cursor, 2);
    assert!(assembly.buffered_output().is_none());
}

#[path = "agent_output_coalesce_recovery_tests.rs"]
mod recovery;

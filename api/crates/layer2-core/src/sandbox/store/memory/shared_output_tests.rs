use super::*;

#[tokio::test]
async fn shared_output_recovers_lost_commit_without_duplicate_content_or_schema_change() {
    let (store, working, _) = working_agent().await;
    let flaky = Arc::new(LostCommitReply {
        store: store.clone(),
        fail_next: AtomicBool::new(true),
        advance_on_read: AtomicBool::new(false),
        publish_on_commit: AtomicBool::new(false),
    });
    let recorder = DurableAgentUpdateRecorder::new(flaky);
    let update = retained_policy(1, Some(message_chunk("reply", "shared exact reply")), true);
    let mut assembly = AgentMessageAssembly::default();
    assert!(matches!(
        recorder
            .record_update(&working.id, 1, &mut assembly, update.clone(), NOW.into())
            .await,
        Err(AgentConversationError::Unavailable(_))
    ));
    let uncertain = serde_json::to_value(&assembly).unwrap();
    assert!(uncertain.get("pending_update").is_none());
    recorder
        .record_update(&working.id, 1, &mut assembly, update.clone(), NOW.into())
        .await
        .unwrap();
    assert_eq!(assembly.content, "shared exact reply");
    assert!(assembly.pending_update.is_none());
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(encoded["version"], 1);
    assert_eq!(encoded["assembly"]["content"], "shared exact reply");
    let restored: AgentMessageCheckpoint = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored, checkpoint);
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut fresh = AgentMessageAssembly::default();
    replacement
        .record_update(&working.id, 1, &mut fresh, update, NOW.into())
        .await
        .unwrap();
    assert_eq!(fresh.content, "shared exact reply");
    replacement
        .record_update(
            &working.id,
            1,
            &mut fresh,
            retained_policy(2, Some(turn_ended()), true),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["shared exact reply"]
    );
}

#[tokio::test]
async fn shared_output_preserves_prompt_and_generation_fences_before_valid_terminal() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            retained_policy(1, Some(message_chunk("reply", "authorized result")), true),
            NOW.into(),
        )
        .await
        .unwrap();
    let checkpoint = store.agent_message_checkpoint(&working.id).await.unwrap();
    let current = store.get_agent(&working.id).await.unwrap().unwrap();
    let terminal = |generation, turn| {
        retained_policy(
            2,
            Some(ConversationUpdate::TurnEndedForPrompt {
                prompt_id: layer3_adapter::PromptIdentity::new("conversation-1", generation, turn),
                response: layer3_adapter::PromptResponse::new(
                    layer3_adapter::TurnStopReason::EndTurn,
                ),
            }),
            true,
        )
    };
    let turn = current.conversation.active_turn.unwrap();
    for update in [terminal(2, turn), terminal(1, turn + 1)] {
        assert!(recorder
            .record_update(&working.id, 1, &mut assembly, update, NOW.into())
            .await
            .is_err());
        assert_eq!(
            store.agent_message_checkpoint(&working.id).await.unwrap(),
            checkpoint
        );
    }
    assert!(matches!(
        recorder
            .record_update(&working.id, 2, &mut assembly, terminal(1, turn), NOW.into())
            .await,
        Err(AgentConversationError::StaleGeneration)
    ));
    assert_eq!(
        store.agent_message_checkpoint(&working.id).await.unwrap(),
        checkpoint
    );
    let outcome = recorder
        .record_update(&working.id, 1, &mut assembly, terminal(1, turn), NOW.into())
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    assert_eq!(
        messages(&store.agent_log(&working.id, 0, 200).await.unwrap()),
        ["authorized result"]
    );
}

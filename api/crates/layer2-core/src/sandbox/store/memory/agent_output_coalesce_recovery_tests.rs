use super::*;

#[tokio::test]
async fn crash_replay_and_disabled_rollback_preserve_durable_logs_assembly_and_turn() {
    let (seed, working, _) = working_agent().await;
    let initial = seed.lock().clone();
    let mut results = Vec::new();
    for mode in ["disabled", "enabled", "crash", "rollback"] {
        // Every run starts from exactly the same IDs and durable history.
        let store = crate::sandbox::store::InMemorySandboxStore::default();
        *store.lock() = initial.clone();
        let disabled = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
        let enabled = recorder(Arc::new(store.clone()), 64);
        let mut assembly = AgentMessageAssembly::default();
        let chosen = if mode == "disabled" {
            &disabled
        } else {
            &enabled
        };
        for cursor in 1..=3 {
            record(chosen.as_ref(), &working.id, &mut assembly, cursor, None).await;
        }
        if mode == "crash" || mode == "rollback" {
            assembly = AgentMessageAssembly::default();
            let replacement = if mode == "rollback" {
                &disabled
            } else {
                &enabled
            };
            replacement
                .restore_assembly(&working.id, 1, &mut assembly)
                .await
                .unwrap();
            for cursor in 1..=3 {
                record(
                    replacement.as_ref(),
                    &working.id,
                    &mut assembly,
                    cursor,
                    None,
                )
                .await;
            }
        }
        let chosen = if mode == "disabled" || mode == "rollback" {
            &disabled
        } else {
            &enabled
        };
        record(
            chosen.as_ref(),
            &working.id,
            &mut assembly,
            4,
            Some(message_chunk("reply", "prefix ")),
        )
        .await;
        record(chosen.as_ref(), &working.id, &mut assembly, 5, None).await;
        record(
            chosen.as_ref(),
            &working.id,
            &mut assembly,
            6,
            Some(message_chunk("reply", "suffix")),
        )
        .await;
        let saved = store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap();
        let encoded = serde_json::to_value(&saved).unwrap();
        assert_eq!(encoded["version"], 1);
        let mut fields: Vec<_> = encoded["assembly"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort();
        assert_eq!(
            fields,
            [
                "content",
                "live_message",
                "live_thought",
                "message_id",
                "prompt_evidence"
            ]
        );
        // The disabled reader restores the exact durable prefix from the same version-1 sidecar.
        let mut old_reader = AgentMessageAssembly::default();
        disabled
            .restore_assembly(&working.id, 1, &mut old_reader)
            .await
            .unwrap();
        assert_eq!(old_reader.content, "prefix suffix");
        let terminal = record(
            chosen.as_ref(),
            &working.id,
            &mut assembly,
            7,
            Some(turn_ended()),
        )
        .await;
        assert!(terminal.outcome.turn_closed);
        let log = store.agent_log(&working.id, 0, 200).await.unwrap();
        results.push((
            log,
            store
                .agent_message_checkpoint(&working.id)
                .await
                .unwrap()
                .unwrap()
                .assembly,
            terminal.outcome.turn_closed,
        ));
    }
    for result in &results[1..] {
        assert_eq!(result, &results[0]);
    }
}

#[tokio::test]
async fn recovered_anchor_cannot_be_deferred_and_legacy_requires_flushing() {
    let (store, working, disabled) = working_agent().await;
    let enabled = recorder(Arc::new(store.clone()), 64);
    let mut assembly = AgentMessageAssembly::default();
    record(
        disabled.as_ref(),
        &working.id,
        &mut assembly,
        1,
        Some(message_chunk("reply", "prefix")),
    )
    .await;
    let saved = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    anchor_tests::publish_connection(&store, &working.id).await;
    // Model an old publisher which did not advance the private sidecar anchor.
    store
        .lock()
        .agent_message_checkpoints
        .insert(working.id.clone(), saved);
    let result = record(enabled.as_ref(), &working.id, &mut assembly, 2, None).await;
    assert_eq!(result.committed.unwrap().cursor, 2);
    record(enabled.as_ref(), &working.id, &mut assembly, 3, None).await;
    assert!(matches!(
        enabled
            .record_output(
                &working.id,
                1,
                &mut assembly,
                message_chunk("reply", " legacy"),
                NOW.into()
            )
            .await,
        Err(AgentConversationError::InvalidState(_))
    ));
    enabled
        .flush_output(&working.id, 1, &mut assembly, NOW.into())
        .await
        .unwrap();
    let result = enabled
        .record_output(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", " legacy"),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(result.committed.is_none());
    assert_eq!(
        store
            .agent_message_checkpoint(&working.id)
            .await
            .unwrap()
            .unwrap()
            .assembly
            .content,
        "prefix legacy"
    );
}

#[tokio::test]
async fn buffered_thoughts_reuse_identity_and_duplicate_frames_do_not_republish() {
    let (store, working, _) = working_agent().await;
    let enabled = recorder(Arc::new(store), 64);
    let mut live = enabled.subscribe_live(&working.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    let thought = || {
        ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::from("fragment"),
        )))
    };
    record(
        enabled.as_ref(),
        &working.id,
        &mut assembly,
        1,
        Some(thought()),
    )
    .await;
    let first = live.try_recv().unwrap();
    record(
        enabled.as_ref(),
        &working.id,
        &mut assembly,
        1,
        Some(thought()),
    )
    .await;
    assert!(live.try_recv().is_err());
    record(
        enabled.as_ref(),
        &working.id,
        &mut assembly,
        2,
        Some(thought()),
    )
    .await;
    assert_eq!(live.try_recv().unwrap().message_id, first.message_id);
}

#[tokio::test]
async fn immediate_recording_cannot_report_a_buffered_replay_as_durable() {
    let (store, working, _) = working_agent().await;
    let enabled = recorder(Arc::new(store), 64);
    let mut assembly = AgentMessageAssembly::default();
    record(enabled.as_ref(), &working.id, &mut assembly, 1, None).await;
    assert!(matches!(
        enabled
            .record_update(&working.id, 1, &mut assembly, retained(1, None), NOW.into())
            .await,
        Err(AgentConversationError::InvalidState(_))
    ));
    assert!(enabled
        .restore_assembly(&working.id, 1, &mut assembly)
        .await
        .is_err());
    assert_eq!(assembly.buffered_output().unwrap().cursor, 1);
}

use super::*;

#[tokio::test]
async fn recorder_observes_actual_fold_effects_without_changing_failure_results() {
    use crate::sandbox::{observe_agent_output, AgentOutputCategory};
    let (_, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    let quiet =
        layer3_adapter::ConversationUpdate::Said(layer3_adapter::SessionUpdate::AgentThoughtChunk(
            layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from("ephemeral")),
        ));
    for (update, category) in [
        (quiet, AgentOutputCategory::QuietReceipt),
        (
            message_chunk("reply", "durable"),
            AgentOutputCategory::DurableAssembly,
        ),
        (turn_ended(), AgentOutputCategory::AppendedLog),
    ] {
        let (result, observed) = observe_agent_output(recorder.record_update(
            &working.id,
            1,
            &mut assembly,
            update,
            NOW.into(),
        ))
        .await;
        result.unwrap();
        assert_eq!(observed, category);
    }
    let (result, category) = observe_agent_output(recorder.record_update(
        &working.id,
        99,
        &mut assembly,
        message_chunk("reply", "refused"),
        NOW.into(),
    ))
    .await;
    assert_eq!(result, Err(AgentConversationError::StaleGeneration));
    assert_eq!(category, AgentOutputCategory::Unclassified);
}

#[tokio::test]
async fn atomic_message_snapshot_matches_persisted_assembly_and_rejects_corruption() {
    let (store, working, recorder) = working_agent().await;
    let empty = store
        .agent_message_snapshot(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(empty.agent, working);
    assert!(empty.checkpoint.is_none());
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            message_chunk("reply", "durable prefix"),
            NOW.into(),
        )
        .await
        .unwrap();
    let snapshot = store
        .agent_message_snapshot(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.agent, working);
    let checkpoint = snapshot.checkpoint.unwrap();
    assert_eq!(checkpoint.assembly.content, "durable prefix");
    assert!(checkpoint.matches(&snapshot.agent));
    assert!(store
        .agent_message_snapshot(&crate::AgentId("absent".into()))
        .await
        .unwrap()
        .is_none());
    store
        .lock()
        .agent_message_checkpoints
        .get_mut(&working.id)
        .unwrap()
        .version = 99;
    assert!(matches!(
        store.agent_message_snapshot(&working.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
}

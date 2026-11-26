use layer3_adapter::{ToolCall, ToolCallStatus};

use super::*;

#[tokio::test]
async fn completed_tool_records_reported_text_without_recording_each_output_delta() {
    let (store, working, _) = working_agent().await;
    let recorder = DurableAgentUpdateRecorder::with_writer_policy(
        Arc::new(store),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all", "accounts":[], "capabilities":["replay_presentation"]
            }))
            .unwrap(),
        ),
    );
    let mut assembly = AgentMessageAssembly::default();
    let update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "proof",
        "status": "completed", "content": [{"type":"content", "content":{"type":"text", "text":"computer-proof"}}]
    })).unwrap();
    let receipt = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(update),
            NOW.into(),
        )
        .await
        .unwrap();
    let events = serde_json::to_value(receipt.appended).unwrap();
    assert_eq!(
        events[0]["kind"]["output"],
        serde_json::json!({"text":"computer-proof", "truncated":false})
    );
}

fn output_update(index: usize) -> ConversationUpdate {
    ConversationUpdate::Said(
        serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "_meta": { "output_delta": { "data": format!("output {index}\n") } }
        }))
        .unwrap(),
    )
}

#[tokio::test]
async fn native_command_completion_records_the_provider_reported_output() {
    let (store, working, _) = working_agent().await;
    let recorder = DurableAgentUpdateRecorder::with_writer_policy(
        Arc::new(store),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all", "accounts":[], "capabilities":["replay_presentation"]
            }))
            .unwrap(),
        ),
    );
    let mut assembly = AgentMessageAssembly::default();
    let update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "native-proof", "status": "completed",
        "rawOutput": {"formatted_output": "native-computer-proof\n", "exit_code": 0}
    }))
    .unwrap();
    let receipt = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(update),
            NOW.into(),
        )
        .await
        .unwrap();
    let events = serde_json::to_value(receipt.appended).unwrap();
    assert_eq!(
        events[0]["kind"]["output"],
        serde_json::json!({"text":"native-computer-proof\n", "truncated":false})
    );
}

#[tokio::test]
async fn reported_output_without_status_is_bounded_and_does_not_invent_a_status() {
    let (store, working, _) = working_agent().await;
    let recorder = DurableAgentUpdateRecorder::with_writer_policy(
        Arc::new(store),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all", "accounts":[], "capabilities":["replay_presentation"]
            }))
            .unwrap(),
        ),
    );
    let mut assembly = AgentMessageAssembly::default();
    let update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "proof",
        "content": [{"type":"content", "content":{"type":"text", "text":"界".repeat(6000)}}]
    }))
    .unwrap();
    let receipt = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(update),
            NOW.into(),
        )
        .await
        .unwrap();
    let events = serde_json::to_value(receipt.appended).unwrap();
    assert_eq!(
        events[0]["kind"],
        serde_json::json!({
            "type":"tool_output_reported", "call_id":"proof",
            "output":{"text":"界".repeat(5461), "truncated":true}
        })
    );
}

#[tokio::test]
async fn output_only_tool_updates_do_not_grow_the_log_before_ordered_completion() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    let started = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::ToolCall(
                ToolCall::new("call-1", "Read repository").status(ToolCallStatus::InProgress),
            )),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        started.appended.as_slice(),
        [AgentLogEntry { kind: AgentLogKind::ToolCallStarted { call_id, status, .. }, .. }]
            if call_id == "call-1" && status == "in_progress"
    ));
    let before = store.get_agent(&working.id).await.unwrap();
    let before_logs = store.agent_log(&working.id, 0, 200).await.unwrap();

    for index in 0..4096 {
        let result = recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                output_update(index),
                NOW.into(),
            )
            .await
            .unwrap();
        assert!(result.appended.is_empty());
        assert!(!result.turn_closed);
    }
    assert_eq!(store.get_agent(&working.id).await.unwrap(), before);
    assert_eq!(
        store.agent_log(&working.id, 0, 200).await.unwrap(),
        before_logs
    );

    for (fields, expected_status, expected_title) in [
        (
            ToolCallUpdateFields::new().title("Read complete"),
            "updated",
            Some("Read complete"),
        ),
        (
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            "completed",
            None,
        ),
    ] {
        let result = recorder
            .record_update(
                &working.id,
                1,
                &mut assembly,
                ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    "call-1", fields,
                ))),
                NOW.into(),
            )
            .await
            .unwrap();
        assert!(matches!(
            result.appended.as_slice(),
            [AgentLogEntry { kind: AgentLogKind::ToolCallUpdated { call_id, status, title, .. }, .. }]
                if call_id == "call-1" && status == expected_status && title.as_deref() == expected_title
        ));
    }
    let completed = recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert!(completed.turn_closed);
    let logs = store.agent_log(&working.id, 0, 200).await.unwrap();
    assert!(logs.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    assert!(
        matches!(logs[logs.len() - 3].kind, AgentLogKind::ToolCallUpdated { ref status, .. } if status == "completed")
    );
    assert!(matches!(
        logs[logs.len() - 2].kind,
        AgentLogKind::TurnCompleted { .. }
    ));
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
}

#[tokio::test]
async fn output_only_tool_update_still_flushes_pending_assistant_text() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            message_chunk("reply", "Visible answer."),
            NOW.into(),
        )
        .await
        .unwrap();
    let result = recorder
        .record_update(&working.id, 1, &mut assembly, output_update(0), NOW.into())
        .await
        .unwrap();
    assert!(
        matches!(result.appended.as_slice(), [AgentLogEntry { kind: AgentLogKind::AgentMessage { content }, .. }] if content == "Visible answer.")
    );
    assert!(assembly.is_empty());
    let after = store.get_agent(&working.id).await.unwrap();
    assert!(recorder
        .record_update(&working.id, 1, &mut assembly, output_update(1), NOW.into())
        .await
        .unwrap()
        .appended
        .is_empty());
    assert_eq!(store.get_agent(&working.id).await.unwrap(), after);
}

#[tokio::test]
async fn output_only_tool_update_preserves_first_message_evidence_once() {
    let (store, agent) = ready_agent(Some("Inspect the proof.")).await;
    let mutation = begin_first_message(&store, agent);
    let working = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();

    let replay = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            output_update(0).without_prompt_evidence(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(replay.appended.is_empty());
    assert_eq!(
        store.get_agent(&working.id).await.unwrap(),
        Some(working.clone())
    );
    let observed = recorder
        .record_update(&working.id, 1, &mut assembly, output_update(1), NOW.into())
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
    let repeated = recorder
        .record_update(&working.id, 1, &mut assembly, output_update(2), NOW.into())
        .await
        .unwrap();
    assert!(repeated.appended.is_empty());
    recorder
        .record_update(&working.id, 1, &mut assembly, turn_ended(), NOW.into())
        .await
        .unwrap();
    assert_eq!(
        store
            .get_agent(&working.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .first_message
            .unwrap()
            .state,
        FirstMessageDelivery::Completed
    );
}

#[tokio::test]
async fn output_only_tool_update_preserves_operation_write_evidence() {
    use crate::sandbox::conversation_fold::{fold_update, FoldedConversationUpdate};
    use crate::sandbox::{AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord};

    let (_, mut working, _) = working_agent().await;
    working.conversation.last_seq += 1;
    let mut operation = AgentOperationRecord::new(
        working.conversation.last_seq,
        1,
        working.conversation.active_turn.unwrap(),
        AgentOperationIntent::steer("Inspect the next file."),
        NOW.into(),
    )
    .unwrap();
    operation.mark_write_confirmed(NOW.into()).unwrap();
    working.conversation.operation = Some(operation);
    let assembly = AgentMessageAssembly::default();

    assert!(matches!(
        fold_update(
            &working,
            &assembly,
            output_update(0).without_prompt_evidence(),
            NOW,
            false,
        )
        .unwrap(),
        FoldedConversationUpdate::Quiet(_)
    ));
    let FoldedConversationUpdate::Changed(change) =
        fold_update(&working, &assembly, output_update(1), NOW, false).unwrap()
    else {
        panic!("owned output must retain operation execution evidence");
    };
    assert!(matches!(
        change.logs.as_slice(),
        [AgentLogKind::OperationDeliveryChanged {
            state: AgentOperationDelivery::ExecutionObserved,
            ..
        }]
    ));
    assert_eq!(
        change.agent.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::ExecutionObserved
    );
    assert!(matches!(
        fold_update(
            &change.agent,
            &change.assembly,
            output_update(2),
            NOW,
            false
        )
        .unwrap(),
        FoldedConversationUpdate::Quiet(_)
    ));
}

#[tokio::test]
async fn output_only_tool_updates_still_reject_stale_generations() {
    let (store, working, recorder) = working_agent().await;
    let mut assembly = AgentMessageAssembly::default();
    for generation in [0, 2] {
        assert_eq!(
            recorder
                .record_update(
                    &working.id,
                    generation,
                    &mut assembly,
                    output_update(0),
                    NOW.into()
                )
                .await,
            Err(AgentConversationError::StaleGeneration)
        );
    }
    assert!(assembly.is_empty());
    assert_eq!(store.get_agent(&working.id).await.unwrap(), Some(working));
}

#[tokio::test]
async fn output_only_tool_updates_still_reject_stopped_agents() {
    let (store, mut stopped, recorder) = working_agent().await;
    stopped.status = AgentStatus::Stopped;
    stopped.stop_reason = Some(AgentStopReason::Owner);
    stopped.stopped_at = Some(NOW.into());
    stopped.conversation.active_turn = None;
    let revision = stopped.revision;
    stopped.revision += 1;
    store.save_agent(revision, stopped.clone()).await.unwrap();
    let mut assembly = AgentMessageAssembly::default();
    assert_eq!(
        recorder
            .record_update(&stopped.id, 1, &mut assembly, output_update(0), NOW.into())
            .await,
        Err(AgentConversationError::InvalidState(
            "conversation updates require a live agent"
        ))
    );
    assert_eq!(store.get_agent(&stopped.id).await.unwrap(), Some(stopped));
}

#[path = "tool_writer_admission_tests.rs"]
mod writer_admission_tests;

#[tokio::test]
async fn file_diffs_on_initial_tool_events_survive_the_status_only_completion() {
    let (store, working, _) = working_agent().await;
    let store = Arc::new(store);
    let recorder = DurableAgentUpdateRecorder::with_writer_policy(
        store.clone(),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all", "accounts":[], "capabilities":["replay_presentation"]
            }))
            .unwrap(),
        ),
    );
    let mut assembly = AgentMessageAssembly::default();
    let started = serde_json::from_value(serde_json::json!({
        "sessionUpdate":"tool_call", "toolCallId":"edit-proof", "title":"Editing files", "kind":"edit", "status":"in_progress",
        "content":[{"type":"diff", "path":"/home/cma/app.js", "oldText":"old\n", "newText":"new\nextra\n"}]
    })).unwrap();
    let start = recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(started),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        start.appended[0].kind,
        AgentLogKind::ToolCallStarted { .. }
    ));
    let AgentLogKind::ToolOutputReported { output, .. } = &start.appended[1].kind else {
        panic!("file diff must be durable");
    };
    assert!(output.text.contains("File: \"/home/cma/app.js\" +2 −1"));
    assert!(output.text.contains("-old\n+new\n+extra\n"));
    assert!(!output.truncated);
    let completed = serde_json::from_value(serde_json::json!({"sessionUpdate":"tool_call_update","toolCallId":"edit-proof","status":"completed"})).unwrap();
    recorder
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(completed),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(store.agent_log(&working.id, 0, 200).await.unwrap().iter().any(|entry| matches!(&entry.kind, AgentLogKind::ToolOutputReported { output, .. } if output.text.contains("+extra"))));
}

#[path = "deliverable_output_tests.rs"]
mod deliverable_output_tests;

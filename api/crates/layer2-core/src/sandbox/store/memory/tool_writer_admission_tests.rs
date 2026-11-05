use super::*;
use crate::sandbox::task_experience::{TaskExperiencePolicy, TaskExperienceWriter};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};

fn policy(accounts: Vec<&str>, capabilities: Vec<&str>) -> Arc<TaskExperiencePolicy> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts", "accounts":accounts, "capabilities":capabilities
        }))
        .unwrap(),
    )
}

fn tool(status: bool) -> ConversationUpdate {
    let mut value = serde_json::json!({
        "sessionUpdate":"tool_call_update", "toolCallId":"call-proof",
        "rawOutput":{"formatted_output":"retained proof"}
    });
    if status {
        value["status"] = "completed".into();
    }
    ConversationUpdate::Said(serde_json::from_value(value).unwrap())
}

// Frozen predecessor shape from a85bc87c: no current event leaf types reused.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PreviousToolEvent {
    ToolCallUpdated {
        call_id: String,
        status: String,
        title: Option<String>,
    },
}

#[tokio::test]
async fn tool_writer_admission_selects_owner_and_capability_and_preserves_old_shape() {
    for (selected, capability, admitted) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let (store, working, _) = working_agent().await;
        let owner = if selected {
            working.owner.as_str()
        } else {
            "another-owner"
        };
        let policy = policy(
            vec![owner],
            if capability {
                vec!["replay_presentation"]
            } else {
                vec!["task_home_admission"]
            },
        );
        assert_eq!(
            policy.allows_writer(&working.owner, TaskExperienceWriter::ToolOutput),
            admitted
        );
        let recorder =
            DurableAgentUpdateRecorder::with_writer_policy(Arc::new(store.clone()), policy);
        let mut assembly = AgentMessageAssembly::default();
        let output = recorder
            .record_update(&working.id, 1, &mut assembly, tool(false), NOW.into())
            .await
            .unwrap();
        assert_eq!(output.appended.len(), usize::from(admitted));
        let status = recorder
            .record_update(&working.id, 1, &mut assembly, tool(true), NOW.into())
            .await
            .unwrap();
        let kind = serde_json::to_value(&status.appended[0].kind).unwrap();
        assert_eq!(kind.get("output").is_some(), admitted);
        let previous = serde_json::from_value::<PreviousToolEvent>(kind.clone());
        if admitted {
            assert!(previous.is_err());
        } else {
            let PreviousToolEvent::ToolCallUpdated {
                call_id,
                status,
                title,
            } = previous.unwrap();
            assert_eq!(
                (call_id.as_str(), status.as_str(), title),
                ("call-proof", "completed", None)
            );
        }
        assert_eq!(
            serde_json::from_value::<AgentLogKind>(kind).unwrap(),
            status.appended[0].kind
        );
        let retained = store.agent_log(&working.id, 0, 200).await.unwrap();
        let closed = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
        closed
            .restore_assembly(&working.id, 1, &mut AgentMessageAssembly::default())
            .await
            .unwrap();
        assert_eq!(
            store.agent_log(&working.id, 0, 200).await.unwrap(),
            retained
        );
    }
}

#[tokio::test]
async fn disabled_tool_output_still_checkpoints_source_receipts_and_rejects_stale_generations() {
    let (store, working, _) = working_agent().await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    let (ack, _) = tokio::sync::mpsc::channel(1);
    let update = ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor: 1,
                native_conversation_id: "conversation-1".into(),
                payload_sha256: format!("{:064x}", 1),
            },
            ack,
        ),
        update: Some(Box::new(tool(false))),
    };
    let outcome = recorder
        .record_update(&working.id, 1, &mut assembly, update.clone(), NOW.into())
        .await
        .unwrap();
    assert!(outcome.appended.is_empty());
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 1);
    let reopened = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    assert!(reopened
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            NOW.into()
        )
        .await
        .unwrap()
        .appended
        .is_empty());
    assert_eq!(
        reopened
            .record_update(&working.id, 2, &mut assembly, tool(true), NOW.into())
            .await,
        Err(AgentConversationError::StaleGeneration)
    );
}

#[tokio::test]
async fn closed_tool_writer_replays_retained_new_output_receipt_without_rewriting_it() {
    let (store, working, _) = working_agent().await;
    let admitted = DurableAgentUpdateRecorder::with_writer_policy(
        Arc::new(store.clone()),
        policy(vec![working.owner.as_str()], vec!["replay_presentation"]),
    );
    let (ack, _) = tokio::sync::mpsc::channel(1);
    let update = ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor: 1,
                native_conversation_id: "conversation-1".into(),
                payload_sha256: format!("{:064x}", 2),
            },
            ack,
        ),
        update: Some(Box::new(tool(false))),
    };
    let written = admitted
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            update.clone(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        written.appended[0].kind,
        AgentLogKind::ToolOutputReported { .. }
    ));
    let before = store.agent_message_checkpoint(&working.id).await.unwrap();
    let closed = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    assert!(closed
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            NOW.into()
        )
        .await
        .unwrap()
        .appended
        .is_empty());
    assert_eq!(
        store.agent_message_checkpoint(&working.id).await.unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_log(&working.id, written.appended[0].seq - 1, 10)
            .await
            .unwrap(),
        written.appended
    );
}

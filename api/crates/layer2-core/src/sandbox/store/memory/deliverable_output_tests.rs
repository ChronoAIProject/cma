use super::*;

#[tokio::test]
async fn deliverable_failure_summary_obeys_output_writer_policy_and_survives_replay() {
    let (store, working, _) = working_agent().await;
    let store = Arc::new(store);
    let with_output = DurableAgentUpdateRecorder::with_writer_policy(
        store.clone(),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all", "accounts":[], "capabilities":["replay_presentation"]
            }))
            .unwrap(),
        ),
    );
    let mut assembly = AgentMessageAssembly::default();
    let text = "This deliverable could not be verified (too_large). Retryable: false. Produce a smaller file and verify its exact path.";
    let result = serde_json::json!({"result":{
        "content":[{"type":"text","text":format!("{text} Synthetic private diagnostic.")}],
        "structuredContent":{"error":{"code":"too_large","retryable":false}}
    },"error":null});
    let input = serde_json::json!({"server":"cma_access","tool":"describe_deliverable"});
    for (id, historical) in [("native", false), ("historical", true)] {
        let started = serde_json::from_value(serde_json::json!({
            "sessionUpdate":"tool_call", "toolCallId":id,
            "title":"Verify an answer deliverable", "kind":"execute",
            "status":if historical {"failed"} else {"in_progress"},
            "rawInput":input, "rawOutput":if historical {result.clone()} else {serde_json::Value::Null}
        })).unwrap();
        with_output
            .record_update(
                &working.id,
                1,
                &mut assembly,
                ConversationUpdate::Said(started),
                NOW.into(),
            )
            .await
            .unwrap();
        if !historical {
            let completion = serde_json::from_value(serde_json::json!({
                "sessionUpdate":"tool_call_update", "toolCallId":id, "status":"failed",
                "rawInput":input, "rawOutput":result,
            }))
            .unwrap();
            with_output
                .record_update(
                    &working.id,
                    1,
                    &mut assembly,
                    ConversationUpdate::Said(completion),
                    NOW.into(),
                )
                .await
                .unwrap();
        }
    }
    let entries = store.agent_log(&working.id, 0, 200).await.unwrap();
    for id in ["native", "historical"] {
        assert!(
            entries.iter().any(|entry| match &entry.kind {
                AgentLogKind::ToolCallUpdated {
                    call_id,
                    output: Some(output),
                    ..
                }
                | AgentLogKind::ToolOutputReported { call_id, output } =>
                    call_id == id && output.text == text && !output.truncated,
                _ => false,
            }),
            "{id}"
        );
    }
    let disabled = DurableAgentUpdateRecorder::new(store.clone());
    let started = serde_json::from_value(serde_json::json!({
        "sessionUpdate":"tool_call","toolCallId":"disabled","status":"failed",
        "title":"Verify an answer deliverable","rawInput":input,"rawOutput":result
    }))
    .unwrap();
    let receipt = disabled
        .record_update(
            &working.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(started),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(receipt
        .appended
        .iter()
        .all(|entry| !matches!(entry.kind, AgentLogKind::ToolOutputReported { .. })));
}

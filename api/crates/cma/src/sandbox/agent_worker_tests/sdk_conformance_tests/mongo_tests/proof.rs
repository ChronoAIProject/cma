use super::*;

pub(super) async fn reject_changed_inputs(router: &Router, agent: &AgentId) {
    for (route, key, _) in INPUTS {
        let response = router
            .clone()
            .oneshot(request(
                route,
                agent,
                key,
                "A changed request must not become new input",
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "{route:?}: {}",
            String::from_utf8_lossy(&body)
        );
    }
}

pub(super) fn assert_inputs(probe: &runtime_tests::RuntimeProbe, agent: &Agent, count: usize) {
    let inputs = probe.inputs();
    assert_eq!(
        inputs.len(),
        count,
        "every runtime write is counted without sender deduplication"
    );
    for (index, (target, input)) in inputs.iter().enumerate() {
        assert_eq!(target.agent_id, agent.id);
        assert_eq!(
            target.conversation_id,
            *agent.conversation.conversation_id.as_ref().unwrap()
        );
        assert_eq!(
            Some(target.runtime_generation),
            agent.conversation.runtime_generation
        );
        assert!(
            matches!(input, ConversationInput::RecoverablePrompt { content: blocks, .. }
            if blocks.len() == 1 && matches!(&blocks[0], layer3_adapter::ContentBlock::Text(text)
                if text.text == INPUTS[index].2)),
            "runtime input {index} preserves public FIFO"
        );
    }
}

pub(super) async fn completed(
    router: &Router,
    store: &MongoSandboxStore,
    worker: &AgentCommandWorker,
    original: &AdmittedAgentCommand,
) {
    let response_id = original.receipt.response_id.as_ref().unwrap();
    let path = format!("/api/v2/responses/{response_id}");
    let public = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response: Value = serde_json::from_str(&read(router, &path).await).unwrap();
            if response["status"] == "completed" {
                return response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("real recorder updates complete the Mongo-backed public response");
    assert_eq!(public["id"], response_id.as_str());
    assert_eq!(
        public["conversation"]["id"],
        original.receipt.envelope.agent_id.as_str()
    );
    assert_eq!(public["output"][0]["content"][0]["text"], "Hello from CMA");
    worker
        .run_one(&original.receipt.envelope.command_id)
        .await
        .unwrap();
    let done = store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.status, AgentResponseStatus::Completed);
}

pub(super) fn ordered_history(logs: &[AgentLogEntry]) {
    assert!(logs.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    let inputs: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::OwnerMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inputs, INPUTS.map(|(_, _, input)| input));
    let starts: Vec<_> = logs
        .iter()
        .filter_map(|entry| match entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => Some(turn_sequence),
            _ => None,
        })
        .collect();
    let ends: Vec<_> = logs
        .iter()
        .filter_map(|entry| match entry.kind {
            AgentLogKind::TurnCompleted { turn_sequence, .. } => Some(turn_sequence),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 3);
    assert_eq!(starts, ends);
    assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::AgentMessage { .. }))
            .count(),
        3
    );
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::ConversationOpened { .. }))
            .count(),
        1
    );
    assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::ToolCallStarted { call_id, .. } if call_id == "sdk-proof")).count(), 1);
    assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::ToolCallUpdated { call_id, status, .. } if call_id == "sdk-proof" && status == "completed")).count(), 1);
    assert!(!serde_json::to_string(logs)
        .unwrap()
        .contains("Checking the proof"));
}

pub(super) async fn replay_and_retry(
    router: &Router,
    store: &MongoSandboxStore,
    worker: &AgentCommandWorker,
    agent: &Agent,
    original: &[AdmittedAgentCommand],
    logs: &[AgentLogEntry],
) {
    for (index, command) in original.iter().enumerate() {
        let response_id = command.receipt.response_id.as_ref().unwrap();
        let done = store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let turn = done.work.dispatch.as_ref().unwrap().turn_sequence;
        for path in [
            format!("/api/v2/responses/{response_id}"),
            format!("/api/v2/responses/{response_id}/events"),
            format!("/api/v1/responses/resp_{}_{turn}", agent.id),
        ] {
            let wire = read(router, &path).await;
            assert!(wire.contains("Hello from CMA"), "{path}");
            assert!(!wire.contains("Checking the proof"));
            assert_eq!(read(router, &path).await, wire, "GET replay remains stable");
        }
        let (route, key, input) = INPUTS[index];
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            router
                .clone()
                .oneshot(request(route, &agent.id, key, input)),
        )
        .await
        .unwrap()
        .unwrap();
        receipt_headers(&response, command);
        drop(response);
        worker
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap(),
            done
        );
    }
    for path in [
        format!("/api/v2/agents/{}/events/snapshot", agent.id),
        format!("/api/v1/agui/events?thread_id={}&follow=false", agent.id),
    ] {
        let wire = read(router, &path).await;
        assert!(wire.contains("Hello from CMA"));
        let mut position = 0;
        for (_, _, input) in INPUTS {
            position += wire[position..]
                .find(input)
                .expect("public replay preserves input order")
                + input.len();
        }
        assert!(!wire.contains("Checking the proof"));
        assert_eq!(read(router, &path).await, wire);
    }
    reject_changed_inputs(router, &agent.id).await;
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
}

use super::*;

#[tokio::test]
async fn orphan_permission_numeric_result_cannot_emit_an_unscoped_terminal() {
    let prompt_id = crate::PromptIdentity::new("conversation-orphan", 1, 1);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let expected_id = prompt_id.as_str().to_owned();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"conversation-orphan"}),
        )
        .await;
        let permission = serde_json::json!({
            "jsonrpc":"2.0", "id":71, "method":"session/request_permission",
            "params": {
                "sessionId":"conversation-orphan",
                "toolCall": {"toolCallId":"tool-1", "title":"Read"},
                "options": [{"optionId":"allow", "name":"Allow", "kind":"allow_once"}],
            },
        });
        send_json(&mut server, permission).await;
        let answer = receive_json(&mut server).await;
        assert_eq!(answer["id"], 71);
        assert_eq!(answer["result"]["outcome"]["outcome"], "selected");
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc":"2.0", "id":999,
                "result":{"stopReason":"end_turn"},
            }),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc":"2.0", "id":expected_id,
                "result":{"stopReason":"end_turn"},
            }),
        )
        .await;
    });
    let (updates, mut received) = mpsc::channel(4);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some("conversation-orphan".into()),
        updates,
    )
    .await
    .expect("bridge opens");
    let observed = tokio::time::timeout(Duration::from_secs(2), received.recv())
        .await
        .expect("permission update arrives")
        .unwrap();
    let request = match observed {
        ConversationUpdate::Asked(request) => request,
        ConversationUpdate::WithoutPromptEvidence(update) => {
            let ConversationUpdate::Asked(request) = *update else {
                panic!("permission request must be observed: {update:?}")
            };
            request
        }
        other => panic!("permission request must be observed: {other:?}"),
    };
    opened
        .channel
        .input
        .send(ConversationInput::PermissionAnswer {
            id: request.id,
            locator: request.locator,
            decision: PermissionDecision::Allow,
        })
        .await
        .expect("durable permission answer writes");
    if let Ok(Some(update)) = tokio::time::timeout(Duration::from_millis(80), received.recv()).await
    {
        panic!("numeric orphan response emitted an update: {update:?}");
    }
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap(),
        ConversationUpdate::TurnEndedForPrompt { prompt_id: received_id, .. }
            if received_id == prompt_id
    ));
    server_task.await.unwrap();
    stop(opened).await;
}

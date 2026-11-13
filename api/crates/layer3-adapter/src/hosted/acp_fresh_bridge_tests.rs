use super::*;

#[tokio::test]
async fn fresh_bridge_routes_startup_prompt_permission_cancel_and_gap_frames() {
    let prompt_id = crate::PromptIdentity::new("conversation-1", 1, 1);
    let expected_prompt_id = prompt_id.clone();
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["protocolVersion"], 1);
        assert_eq!(
            initialize.pointer("/params/clientCapabilities/_meta/jetbrains/air/capabilities"),
            Some(&serde_json::json!(["sessionFailure"]))
        );

        let startup = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
            "startup activity",
        )));
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "conversation-1", "update": startup },
            }),
        )
        .await;
        respond(&mut server, &initialize, serde_json::json!({})).await;

        let new_session = receive_json(&mut server).await;
        assert_eq!(new_session["method"], "session/new");
        assert_eq!(new_session["params"]["mcpServers"], serde_json::json!([]));
        assert_eq!(new_session["params"]["cwd"], "/workspace");
        respond(
            &mut server,
            &new_session,
            serde_json::json!({ "sessionId": "conversation-1" }),
        )
        .await;

        let mode = receive_json(&mut server).await;
        assert_eq!(mode["method"], "session/set_mode");
        assert_eq!(mode["params"]["sessionId"], "conversation-1");
        assert_eq!(mode["params"]["modeId"], "read-only");
        respond(&mut server, &mode, serde_json::json!({})).await;

        let prompt = receive_json(&mut server).await;
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], "conversation-1");
        assert_eq!(prompt["id"], expected_prompt_id.as_str());

        let options = vec![
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
        ];
        let wrong_session = RequestPermissionRequest::new(
            "conversation-other",
            ToolCallUpdate::new("tool-1", ToolCallUpdateFields::default()),
            options.clone(),
        );
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 70,
                "method": "session/request_permission", "params": wrong_session,
            }),
        )
        .await;
        let refused = receive_json(&mut server).await;
        assert_eq!(refused["id"], 70);
        assert_eq!(refused["error"]["code"], -32602);

        let permission = RequestPermissionRequest::new(
            "conversation-1",
            ToolCallUpdate::new("tool-1", ToolCallUpdateFields::default()),
            options,
        );
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 71,
                "method": "session/request_permission", "params": permission,
            }),
        )
        .await;
        let first = receive_json(&mut server).await;
        let repeated = receive_json(&mut server).await;
        assert_eq!(first, repeated, "repeated durable answer is idempotent");
        assert_eq!(first["id"], 71);
        assert_eq!(first["result"]["outcome"]["outcome"], "selected");
        assert_eq!(first["result"]["outcome"]["optionId"], "allow");

        respond(
            &mut server,
            &prompt,
            serde_json::to_value(PromptResponse::new(TurnStopReason::EndTurn))
                .expect("prompt response serializes"),
        )
        .await;
        let mut gap = vec![LANE_CONVERSATION_GAP];
        gap.extend_from_slice(br#"{"frames":4}"#);
        server
            .send(Message::Binary(gap))
            .await
            .expect("gap frame sends");

        let cancel = receive_json(&mut server).await;
        assert_eq!(cancel["method"], "session/cancel");
        assert_eq!(cancel["params"]["sessionId"], "conversation-1");
    });

    let (updates_tx, mut updates) = mpsc::channel(16);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(Some("read-only"), false),
        None,
        updates_tx,
    )
    .await
    .expect("fresh bridge opens");
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-1")
    );
    assert!(matches!(
        updates.recv().await.expect("startup update"),
        ConversationUpdate::WithoutPromptEvidence(update)
            if matches!(*update, ConversationUpdate::Said(_))
    ));

    opened
        .channel
        .input
        .send(ConversationInput::RecoverablePrompt {
            content: vec![ContentBlock::from("hello")],
            prompt_id,
        })
        .await
        .expect("prompt writes");
    let asked = updates.recv().await.expect("permission update");
    let ConversationUpdate::Asked(asked) = asked else {
        panic!("expected permission update, received {asked:?}");
    };
    let locator = asked.locator.expect("permission carries durable locator");
    let answer = ConversationInput::PermissionAnswer {
        id: asked.id,
        locator: Some(locator),
        decision: PermissionDecision::Allow,
    };
    opened
        .channel
        .input
        .send(answer.clone())
        .await
        .expect("permission answer writes");
    opened
        .channel
        .input
        .send(answer)
        .await
        .expect("repeated permission answer writes");
    assert!(matches!(
        updates.recv().await.expect("turn result"),
        ConversationUpdate::TurnEndedForPrompt { .. }
    ));
    assert!(matches!(
        updates.recv().await.expect("gap update"),
        ConversationUpdate::Lost { frames: 4 }
    ));
    opened
        .channel
        .input
        .send(ConversationInput::Cancel)
        .await
        .expect("cancel writes");

    server.await.expect("ACP fixture completes");
    stop(opened).await;
}

use super::*;

#[tokio::test]
async fn ordinary_session_does_not_receive_channel_effort_override() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        assert_eq!(open["params"]["mcpServers"], serde_json::json!([]));
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"ordinary-conversation"}),
        )
        .await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(
                !matches!(frame, Message::Binary(_)),
                "ordinary session cannot receive effort override"
            );
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        None,
        updates,
    )
    .await
    .unwrap();
    stop(opened).await;
    serving.await.unwrap();
}

#[tokio::test]
async fn unsupported_or_unconfirmed_channel_effort_does_not_prevent_session_opening() {
    for response in [
        None,
        Some(serde_json::json!({"configOptions":[]})),
        Some(serde_json::json!({
            "configOptions":[{"id":"reasoning_effort", "currentValue":"xhigh", "options":[{"value":"low"}]}]
        })),
    ] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            respond(
                &mut server,
                &initialize,
                serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}),
            )
            .await;
            let open = receive_json(&mut server).await;
            respond(
                &mut server,
                &open,
                serde_json::json!({"sessionId":"channel-conversation"}),
            )
            .await;
            let config = receive_json(&mut server).await;
            assert_eq!(config["method"], "session/set_config_option");
            match response {
                Some(response) => respond(&mut server, &config, response).await,
                None => reject(&mut server, &config).await,
            }
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "no retry or fabricated prompt"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        configured.channel_tool = Some(binding(None));
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            None,
            updates,
        )
        .await
        .unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("channel-conversation")
        );
        stop(opened).await;
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn unanswered_channel_effort_is_bounded_and_preserves_open_session() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(
            &mut server,
            &initialize,
            serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}),
        )
        .await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"channel-conversation"}),
        )
        .await;
        let config = receive_json(&mut server).await;
        assert_eq!(config["method"], "session/set_config_option");
        let _ = server.next().await;
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.channel_tool = Some(binding(None));
    let opened = tokio::time::timeout(
        Duration::from_secs(2),
        Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            None,
            updates,
        ),
    )
    .await
    .expect("optional effort cannot hold delivery open")
    .unwrap();
    stop(opened).await;
    serving.await.unwrap();
}

use super::*;

#[tokio::test]
async fn recovered_provider_failure_keeps_identity_and_emits_once_without_resending() {
    for metadata in [false, true] {
        let prompt_id = crate::PromptIdentity::new("conversation-held", 17, 8);
        let expected = prompt_id.clone();
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let (release, held) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            assert_eq!(initialize["method"], "initialize");
            reject(&mut server, &initialize).await;
            let message = "unexpected status 524 A Timeout Occurred: PRIVATE_BODY_SENTINEL";
            let mut terminal = if metadata {
                serde_json::json!({"result":{"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"sessionFailure":{"severity":"error","title":message}}}}}})
            } else {
                serde_json::json!({"error":{"message":message,"data":{"capability":"PRIVATE_CLASS_SENTINEL"}}})
            };
            terminal["jsonrpc"] = "2.0".into();
            terminal["id"] = expected.as_str().into();
            send_json(&mut server, terminal.clone()).await;
            send_json(&mut server, terminal).await;
            tokio::select! {
                _ = held => {}
                unexpected = server.next() => panic!("recovery resent input: {unexpected:?}"),
            }
        });
        let (updates, mut received) = mpsc::channel(4);
        let opened = Bridge::open_with_subscription_and_prompt(
            client,
            "agent-1",
            "/workspace".into(),
            (settings(None, false), None),
            Some("conversation-held".into()),
            Some(prompt_id.clone()),
            updates,
        )
        .await
        .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(outcome, ConversationUpdate::FailedForPrompt { prompt_id: actual, capability, message }
            if actual == prompt_id && capability.as_deref() == Some("upstream_timeout")
                && message.starts_with("The upstream model service timed out.") && !message.contains("SENTINEL"))
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(40), received.recv())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        server_task.await.unwrap();
        stop(opened).await;
    }
}

#[tokio::test]
async fn unwritten_prompt_emits_only_safe_failure_and_keeps_send_disposition() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (release, held) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"original"}),
        )
        .await;
        tokio::select! {
            _ = held => {}
            unexpected = server.next() => panic!("unwritten input reached socket: {unexpected:?}"),
        }
    });
    let (updates, mut received) = mpsc::channel(4);
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
    opened.bridge.retired.store(true, Ordering::SeqCst);
    let prompt_id = crate::PromptIdentity::new("original", 1, 2);
    let sent = opened
        .channel
        .input
        .send_classified(ConversationInput::RecoverablePrompt {
            content: vec![ContentBlock::from("PRIVATE_INPUT_SENTINEL")],
            prompt_id: prompt_id.clone(),
        })
        .await;
    assert!(matches!(
        sent,
        crate::ConversationSendOutcome::PreWriteFailure(crate::AdapterError::PromptNotWritten(_))
    ));
    let update = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(update, ConversationUpdate::FailedForPrompt { prompt_id: actual, capability, message }
        if actual == prompt_id && capability.as_deref() == Some("agent_turn_failed")
            && message.starts_with("The Agent run failed.") && !message.contains("SENTINEL"))
    );
    assert_eq!(opened.bridge.owned_prompt_waiters.load(Ordering::SeqCst), 0);
    assert!(opened.bridge.pending.lock().await.is_empty());
    release.send(()).unwrap();
    server_task.await.unwrap();
    stop(opened).await;
}

use super::*;

#[tokio::test]
async fn retired_prompt_is_proven_unwritten_at_both_writer_guards() {
    for during_admission in [false, true] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let (release, held) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
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
                _ = held => {},
                frame = server.next() => panic!("retired prompt reached the socket: {frame:?}"),
            }
        });
        let (updates, _receiver) = mpsc::channel(4);
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
        if during_admission {
            // Pause after the first retired check but before the frame writer.
            let pending = opened.bridge.pending.lock().await;
            let attempt = opened.bridge.begin_prompt(
                serde_json::json!({"sessionId":"original","prompt":[]}),
                None,
            );
            tokio::pin!(attempt);
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut attempt)
                    .await
                    .is_err()
            );
            opened.bridge.retired.store(true, Ordering::SeqCst);
            drop(pending);
            assert!(matches!(
                attempt.await,
                Err(crate::AdapterError::PromptNotWritten(_))
            ));
        } else {
            opened.bridge.retired.store(true, Ordering::SeqCst);
            let result = opened
                .channel
                .input
                .send_classified(ConversationInput::RecoverablePrompt {
                    content: vec![ContentBlock::from("must remain unwritten")],
                    prompt_id: crate::PromptIdentity::new("original", 1, 2),
                })
                .await;
            assert!(matches!(
                result,
                crate::ConversationSendOutcome::PreWriteFailure(
                    crate::AdapterError::PromptNotWritten(_)
                )
            ));
        }
        assert!(opened.bridge.pending.lock().await.is_empty());
        release.send(()).unwrap();
        server.await.unwrap();
        stop(opened).await;
    }
}

#[tokio::test]
async fn prompt_prewrite_category_does_not_reclassify_other_operations_or_transport_errors() {
    let input = crate::ConversationInputSender::spawn(1, |_| async {
        Err(crate::AdapterError::PromptNotWritten(
            "later RPC refused".into(),
        ))
    });
    assert!(matches!(
        input.send_classified(ConversationInput::Cancel).await,
        crate::ConversationSendOutcome::Ambiguous(_)
    ));
    let input = crate::ConversationInputSender::spawn(1, |_| async {
        Err(crate::AdapterError::Unavailable(
            "conversation write failed: connection closed".into(),
        ))
    });
    assert!(matches!(
        input
            .send_classified(ConversationInput::Prompt(vec![ContentBlock::from(
                "maybe written"
            )]))
            .await,
        crate::ConversationSendOutcome::Ambiguous(_)
    ));
}

/// The runtime still owns the original native prompt after its API socket is
/// replaced. The new adapter must consume that prompt's result without sending
/// the user input again or requiring an unrelated permission answer.
#[tokio::test]
async fn replacement_bridge_recovers_plain_prompt_result_without_resending() {
    let prompt_id = crate::PromptIdentity::new("conversation-held", 17, 8);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (sent, received) = tokio::sync::oneshot::channel();
    let (release, held) = tokio::sync::oneshot::channel();
    let original_server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let session = receive_json(&mut server).await;
        assert_eq!(session["method"], "session/new");
        respond(
            &mut server,
            &session,
            serde_json::json!({"sessionId": "conversation-held"}),
        )
        .await;
        let prompt = receive_json(&mut server).await;
        assert_eq!(prompt["method"], "session/prompt");
        sent.send(prompt).expect("original prompt observed");
        let _ = held.await;
    });
    let (updates, mut old_updates) = mpsc::channel(4);
    let original = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        None,
        updates,
    )
    .await
    .expect("original connection opens");
    original
        .channel
        .input
        .send(ConversationInput::RecoverablePrompt {
            content: vec![ContentBlock::from("One harmless answer, without tools.")],
            prompt_id: prompt_id.clone(),
        })
        .await
        .expect("exactly one prompt writes");
    let prompt = received.await.expect("native prompt remains active");
    assert_eq!(prompt["id"], prompt_id.as_str());
    // Keep the old update relay alive while its socket is explicitly retired:
    // dropping a waiter is transport loss, not evidence of model failure.
    original.bridge.retire().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(40), old_updates.recv())
            .await
            .is_err()
    );
    stop(original).await;
    let _ = release.send(());
    original_server.await.expect("original socket closed");

    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (release, held) = tokio::sync::oneshot::channel();
    let replacement_server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        reject(&mut server, &initialize).await;
        respond(
            &mut server,
            &prompt,
            serde_json::to_value(PromptResponse::new(TurnStopReason::EndTurn))
                .expect("prompt result serializes"),
        )
        .await;
        tokio::select! {
            _ = held => {}
            unexpected = server.next() => panic!("recovery resent native input: {unexpected:?}"),
        }
    });
    let (updates, mut recovered) = mpsc::channel(4);
    let replacement = Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(prompt_id),
        updates,
    )
    .await
    .expect("replacement reconnects the retained native conversation");
    let outcome = tokio::time::timeout(Duration::from_secs(1), recovered.recv()).await;
    let _ = release.send(());
    replacement_server.await.expect("replacement socket served");
    stop(replacement).await;
    assert!(
        matches!(
            outcome,
            Ok(Some(ConversationUpdate::TurnEndedForPrompt { ref prompt_id, .. }))
                if prompt_id == &crate::PromptIdentity::new("conversation-held", 17, 8)
        ),
        "retained prompt result must close the original turn with its identity, received {outcome:?}"
    );
}

#[tokio::test]
async fn waiterless_prompt_response_keeps_exact_identity_and_is_idempotent() {
    let prompt_id = crate::PromptIdentity::new("conversation-waiterless", 3, 11);
    let response_id = prompt_id.as_str().to_owned();
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (prompt_seen_tx, prompt_seen_rx) = tokio::sync::oneshot::channel();
    let (prompt_ready_tx, prompt_ready_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let open = receive_json(&mut server).await;
        respond(
            &mut server,
            &open,
            serde_json::json!({"sessionId":"conversation-waiterless"}),
        )
        .await;
        let prompt = receive_json(&mut server).await;
        assert_eq!(prompt["id"], response_id);
        prompt_seen_tx.send(()).unwrap();
        prompt_ready_rx.await.unwrap();
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": prompt["id"].clone(),
            "result": {"stopReason": "end_turn"},
        });
        send_json(&mut server, response.clone()).await;
        send_json(&mut server, response).await;
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
    .expect("bridge opens");
    opened
        .channel
        .input
        .send(ConversationInput::RecoverablePrompt {
            content: vec![ContentBlock::from("waiterless")],
            prompt_id: prompt_id.clone(),
        })
        .await
        .expect("prompt sends");
    prompt_seen_rx.await.unwrap();
    // Simulate the request waiter being lost while the attachment remains
    // alive. The bridge keeps the admitted identity for the response path.
    opened
        .bridge
        .pending
        .lock()
        .await
        .remove(prompt_id.as_str());
    prompt_ready_tx.send(()).unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .expect("waiterless terminal arrives")
        .expect("update relay remains open");
    assert!(matches!(
        terminal,
        ConversationUpdate::TurnEndedForPrompt { prompt_id: received_id, .. }
            if received_id == prompt_id
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(80), received.recv())
            .await
            .is_err()
    );
    server_task.await.unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn recovered_terminal_waits_for_handshake_and_routes_exact_failure_once() {
    let expected = crate::PromptIdentity::new("conversation-held", 17, 8);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (sent, observed) = tokio::sync::oneshot::channel();
    let (verify, verified) = tokio::sync::oneshot::channel();
    let (release, held) = tokio::sync::oneshot::channel();
    let rpc_id = expected.as_str().to_owned();
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        assert!(initialize["id"].as_str().unwrap().starts_with("cma-rpc-"));
        let outcome = serde_json::json!({
            "jsonrpc": "2.0", "id": rpc_id,
            "error": {"code": -32000, "message": "provider failed after restart"},
        });
        send_json(&mut server, outcome.clone()).await;
        sent.send(()).unwrap();
        verified.await.unwrap();
        reject(&mut server, &initialize).await;
        send_json(&mut server, outcome).await;
        tokio::select! {
            _ = held => {}
            unexpected = server.next() => panic!("recovery resent native input: {unexpected:?}"),
        }
    });
    let (updates, mut recovered) = mpsc::channel(4);
    let opening = tokio::spawn(Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(expected),
        updates,
    ));
    observed.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), recovered.recv())
            .await
            .is_err()
    );
    verify.send(()).unwrap();
    let opened = opening.await.unwrap().expect("held session verifies");
    let outcome = tokio::time::timeout(Duration::from_secs(1), recovered.recv())
        .await
        .expect("exact recovered failure arrives")
        .expect("update relay remains open");
    assert!(matches!(
        outcome,
        ConversationUpdate::FailedForPrompt { prompt_id, message, .. }
            if prompt_id == crate::PromptIdentity::new("conversation-held", 17, 8)
                && message == "provider failed after restart"
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(40), recovered.recv())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    server.await.unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn recovery_ignores_other_turns_generations_sessions_and_legacy_numbers() {
    let expected = crate::PromptIdentity::new("conversation-held", 17, 8);
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (release, held) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        for id in [
            serde_json::json!(1),
            serde_json::json!(crate::PromptIdentity::new("conversation-other", 17, 8).as_str()),
            serde_json::json!(crate::PromptIdentity::new("conversation-held", 16, 8).as_str()),
            serde_json::json!(crate::PromptIdentity::new("conversation-held", 17, 7).as_str()),
            serde_json::json!("unrelated"),
        ] {
            send_json(
                &mut server,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"},
                }),
            )
            .await;
        }
        reject(&mut server, &initialize).await;
        let _ = held.await;
    });
    let (updates, mut recovered) = mpsc::channel(4);
    let opened = Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(expected),
        updates,
    )
    .await
    .expect("unrelated results cannot satisfy initialize");
    assert!(
        tokio::time::timeout(Duration::from_millis(40), recovered.recv())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    server.await.unwrap();
    stop(opened).await;
}

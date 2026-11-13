use super::*;

const CONVERSATION: &str = "conversation-held";

async fn open_fixture(
    reliable: bool,
    resume: bool,
) -> (
    ServerSocket,
    tokio::task::JoinHandle<crate::Result<OpenedBridge>>,
    tokio::task::JoinHandle<()>,
) {
    let (client, mut server) = socket_pair().await;
    if reliable {
        output_tests::reliable_ready(&mut server).await;
    } else {
        send_ready(&mut server, "agent-1").await;
    }
    let (updates, mut receiver) = mpsc::channel(8);
    let recording = tokio::spawn(async move {
        while let Some(update) = receiver.recv().await {
            if let ConversationUpdate::RuntimeOutput { receipt, .. } = update {
                receipt.acknowledge().await.unwrap();
            }
        }
    });
    let opening = tokio::spawn(Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        resume.then(|| CONVERSATION.into()),
        updates,
    ));
    (server, opening, recording)
}

async fn reply(
    server: &mut ServerSocket,
    reliable: bool,
    cursor: u64,
    request: &serde_json::Value,
    existing: bool,
) {
    let mut frame = serde_json::json!({"jsonrpc":"2.0", "id":request["id"]});
    if existing {
        frame["error"] = settings(None, false).already_initialized_error.unwrap();
    } else {
        frame["result"] = serde_json::json!({});
    }
    if reliable {
        output_tests::retained(server, cursor, frame).await;
    } else {
        send_json(server, frame).await;
    }
}

async fn next_call(server: &mut ServerSocket) -> serde_json::Value {
    loop {
        let Message::Binary(bytes) = server.next().await.unwrap().unwrap() else {
            panic!("expected an open conversation socket");
        };
        if bytes[0] == LANE_CONVERSATION {
            return serde_json::from_slice(&bytes[1..]).unwrap();
        }
        assert_eq!(bytes[0], LANE_OUTPUT_ACK);
    }
}

async fn assert_slow_recovery_survives(
    opening: &tokio::task::JoinHandle<crate::Result<OpenedBridge>>,
) {
    elapse_silent_recovery().await;
    assert!(
        !opening.is_finished(),
        "a silent recovery must outlive the ordinary RPC deadline"
    );
}

#[tokio::test]
async fn silent_cold_restore_keeps_the_original_conversation_socket() {
    for reliable in [false, true] {
        let (mut server, opening, recording) = open_fixture(reliable, true).await;
        let initialize = next_call(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        assert_slow_recovery_survives(&opening).await;
        reply(&mut server, reliable, 1, &initialize, false).await;

        let load = next_call(&mut server).await;
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["sessionId"], CONVERSATION);
        assert_slow_recovery_survives(&opening).await;
        reply(&mut server, reliable, 2, &load, false).await;

        let opened = opening.await.unwrap().unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some(CONVERSATION)
        );
        stop(opened).await;
        recording.abort();
    }
}

#[tokio::test]
async fn silent_initialized_bridge_reconnects_without_reloading_history() {
    for reliable in [false, true] {
        let (mut server, opening, recording) = open_fixture(reliable, true).await;
        let initialize = next_call(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        assert_slow_recovery_survives(&opening).await;
        reply(&mut server, reliable, 1, &initialize, true).await;
        let opened = opening.await.unwrap().unwrap();
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some(CONVERSATION)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), next_call(&mut server))
                .await
                .is_err()
        );
        stop(opened).await;
        recording.abort();
    }
}

#[tokio::test]
async fn silent_recovery_still_has_a_bounded_idle_deadline() {
    for reliable in [false, true] {
        for stall_initialize in [false, true] {
            let (mut server, opening, recording) = open_fixture(reliable, true).await;
            let initialize = next_call(&mut server).await;
            if !stall_initialize {
                reply(&mut server, reliable, 1, &initialize, false).await;
                assert_eq!(next_call(&mut server).await["method"], "session/load");
            }
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(299)).await;
            tokio::task::yield_now().await;
            assert!(!opening.is_finished());
            tokio::time::advance(Duration::from_secs(2)).await;
            tokio::task::yield_now().await;
            assert!(
                opening.is_finished(),
                "stalled recovery must return for retry"
            );
            assert!(matches!(
                opening.await.unwrap(),
                Err(AdapterError::Unavailable(_))
            ));
            tokio::time::resume();
            recording.abort();
        }
    }
}

#[tokio::test]
async fn fresh_initialize_retains_the_ordinary_rpc_deadline() {
    for reliable in [false, true] {
        let (mut server, opening, recording) = open_fixture(reliable, false).await;
        assert_eq!(next_call(&mut server).await["method"], "initialize");
        tokio::time::pause();
        tokio::time::advance(CALL_DEADLINE + Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(opening.is_finished());
        assert!(matches!(
            opening.await.unwrap(),
            Err(AdapterError::Unavailable(_))
        ));
        tokio::time::resume();
        recording.abort();
    }
}

#[tokio::test]
async fn resumed_control_calls_retain_the_ordinary_rpc_deadline() {
    let (mut server, opening, recording) = open_fixture(false, true).await;
    let initialize = next_call(&mut server).await;
    reply(&mut server, false, 1, &initialize, true).await;
    let opened = opening.await.unwrap().unwrap();
    let bridge = opened.bridge.clone();
    let calling = tokio::spawn(async move {
        bridge
            .call(
                "session/set_mode",
                serde_json::json!({
                    "sessionId": CONVERSATION, "modeId": "default",
                }),
            )
            .await
    });
    assert_eq!(next_call(&mut server).await["method"], "session/set_mode");
    tokio::time::pause();
    tokio::time::advance(CALL_DEADLINE + Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(calling.is_finished());
    assert!(matches!(
        calling.await.unwrap(),
        Err(AdapterError::Unavailable(_))
    ));
    tokio::time::resume();
    stop(opened).await;
    recording.abort();
}

#[tokio::test]
async fn disconnected_restore_returns_for_retry_without_waiting_for_the_idle_deadline() {
    for reliable in [false, true] {
        let (mut server, opening, recording) = open_fixture(reliable, true).await;
        let initialize = next_call(&mut server).await;
        reply(&mut server, reliable, 1, &initialize, false).await;
        assert_eq!(next_call(&mut server).await["method"], "session/load");
        drop(server);
        let result = tokio::time::timeout(Duration::from_secs(2), opening)
            .await
            .expect("a disconnected bridge must not consume the recovery idle budget")
            .unwrap();
        assert!(matches!(result, Err(AdapterError::Unavailable(_))));
        recording.abort();
    }
}

// Advance only after receiving the request; keep socket I/O on the real clock.
pub(super) async fn elapse_silent_recovery() {
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
}

#[tokio::test]
async fn fresh_session_new_retains_the_ordinary_rpc_deadline() {
    for reliable in [false, true] {
        let (mut server, opening, recording) = open_fixture(reliable, false).await;
        let initialize = next_call(&mut server).await;
        reply(&mut server, reliable, 1, &initialize, false).await;
        assert_eq!(next_call(&mut server).await["method"], "session/new");
        tokio::time::pause();
        tokio::time::advance(CALL_DEADLINE + Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(opening.is_finished());
        assert!(matches!(
            opening.await.unwrap(),
            Err(AdapterError::Unavailable(_))
        ));
        tokio::time::resume();
        recording.abort();
    }
}

#[tokio::test]
async fn recovery_ack_progress_renews_idle_but_keeps_the_absolute_call_bound() {
    for stall_initialize in [false, true] {
        let (mut server, opening, recording) = open_fixture(true, true).await;
        let initialize = next_call(&mut server).await;
        let first_cursor = if stall_initialize {
            1
        } else {
            reply(&mut server, true, 1, &initialize, false).await;
            assert_eq!(next_call(&mut server).await["method"], "session/load");
            2
        };
        for cursor in first_cursor..first_cursor + 3 {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(240)).await;
            tokio::task::yield_now().await;
            assert!(!opening.is_finished());
            tokio::time::resume();
            output_tests::retained(
                &mut server,
                cursor,
                serde_json::json!({
                    "jsonrpc":"2.0", "method":"cma/noop"
                }),
            )
            .await;
            loop {
                let Message::Binary(frame) = server.next().await.unwrap().unwrap() else {
                    panic!("expected durable ACK");
                };
                assert_eq!(frame[0], LANE_OUTPUT_ACK);
                let ack_cursor = u64::from_be_bytes(frame[17..25].try_into().unwrap());
                if ack_cursor == cursor {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(241)).await;
        tokio::task::yield_now().await;
        assert!(
            opening.is_finished(),
            "ACKs must not extend the sixteen-minute bound"
        );
        assert!(
            matches!(opening.await.unwrap(), Err(AdapterError::Unavailable(detail)) if detail.contains("recovery deadline"))
        );
        tokio::time::resume();
        recording.abort();
    }
}

#[tokio::test]
async fn incomplete_rejoin_prompt_refusal_retains_terminal_identity_and_allows_retry() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (updates, mut received) = mpsc::channel(8);
    let opening = tokio::spawn(Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some(CONVERSATION.into()),
        updates,
    ));
    let initialize = next_call(&mut server).await;
    reply(&mut server, false, 1, &initialize, true).await;
    let opened = opening.await.unwrap().unwrap();
    for sequence in 1..=2 {
        let prompt_id = crate::PromptIdentity::new(CONVERSATION, sequence, sequence);
        opened
            .channel
            .input
            .send(ConversationInput::RecoverablePrompt {
                content: vec![ContentBlock::from("retained owner input")],
                prompt_id: prompt_id.clone(),
            })
            .await
            .unwrap();
        let prompt = next_call(&mut server).await;
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], CONVERSATION);
        if sequence == 1 {
            // Exact envelope from the pinned bridge before native installation
            // and after a failed load; it must never look like a successful turn.
            send_json(
                &mut server,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": prompt["id"], "error": {
                        "code": -32603, "message": "Internal error",
                        "data": {"details": format!("Session {CONVERSATION} not found")}
                    }
                }),
            )
            .await;
            assert!(matches!(received.recv().await.unwrap(),
                ConversationUpdate::FailedForPrompt { prompt_id: actual, message, capability: None }
                if actual == prompt_id && message == "Internal error"));
        } else {
            respond(
                &mut server,
                &prompt,
                serde_json::json!({"stopReason":"end_turn"}),
            )
            .await;
            assert!(matches!(received.recv().await.unwrap(),
                ConversationUpdate::TurnEndedForPrompt { prompt_id: actual, .. }
                if actual == prompt_id));
        }
    }
    stop(opened).await;
}

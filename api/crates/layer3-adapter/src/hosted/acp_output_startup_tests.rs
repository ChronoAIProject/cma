use super::*;

#[tokio::test]
async fn retained_backlog_over_256_keeps_terminal_and_receipts_ordered_through_handshake() {
    for shared in [false, true] {
        retained_backlog_with_ownership(shared).await;
    }
}

async fn retained_backlog_with_ownership(shared: bool) {
    let prompt = crate::PromptIdentity::new("conversation-held", 19, 157);
    let expected_id = prompt.as_str().to_owned();
    let (client, mut server) = socket_pair().await;
    reliable_ready(&mut server).await;
    let (backlog_sent, backlog_received) = tokio::sync::oneshot::channel();
    let (verify, verified) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        for cursor in 1..=300 {
            retained(&mut server, cursor, serde_json::json!({
                "jsonrpc": "2.0", "method": "session/update", "params": {
                    "sessionId": "conversation-held", "update": {
                        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "x"},
                    },
                },
            })).await;
        }
        retained(
            &mut server,
            301,
            serde_json::json!({
                "jsonrpc": "2.0", "id": expected_id, "result": {"stopReason": "end_turn"},
            }),
        )
        .await;
        backlog_sent.send(()).unwrap();
        verified.await.unwrap();
        retained(&mut server, 302, serde_json::json!({
            "jsonrpc": "2.0", "id": initialize["id"], "error": {
                "code": -32603, "message": "Internal error", "data": {"details": "Already initialized"}
            },
        })).await;
        let message = tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Binary(bytes) = message else {
            panic!("expected acknowledgement")
        };
        assert_eq!(bytes[0], LANE_OUTPUT_ACK);
        assert_eq!(&bytes[1..17], stream().as_bytes());
        assert_eq!(u64::from_be_bytes(bytes[17..25].try_into().unwrap()), 1);
    });
    let (updates, mut received) = mpsc::channel(4);
    let mut configured = settings(None, false);
    configured.shared_runtime_output = shared;
    let opening = tokio::spawn(Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (configured, None),
        Some("conversation-held".into()),
        Some(prompt.clone()),
        updates,
    ));
    backlog_received.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), received.recv())
            .await
            .is_err()
    );
    verify.send(()).unwrap();
    let opened = opening
        .await
        .unwrap()
        .expect("backlog does not block handshake RPC");
    let mut first_receipt = None;
    for cursor in 1..=302 {
        let next = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, update } = next else {
            panic!("all frames must retain their receipt")
        };
        assert_eq!(receipt.source.cursor, cursor);
        if let Some(effect) = &update {
            assert_eq!(effect.is_shared(), shared);
            assert_eq!(std::ptr::eq(&**effect, &*effect.clone()), shared);
        }
        assert_eq!(receipt.source.native_conversation_id, "conversation-held");
        if cursor == 301 {
            assert!(matches!(
                update.as_deref(),
                Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. })
                    if prompt_id == &prompt
            ));
        } else if cursor == 302 {
            assert!(update.is_none());
        } else {
            assert!(matches!(
                update.as_deref(),
                Some(ConversationUpdate::WithoutPromptEvidence(_))
            ));
        }
        if cursor == 1 {
            first_receipt = Some(receipt);
        }
    }
    // Enqueueing and parsing all 302 frames did not acknowledge anything.
    use crate::ConversationSettingsSupport;
    let mut progress = opened.bridge.managed_context_progress().await.unwrap();
    assert_eq!(*progress.borrow(), 0);
    first_receipt.unwrap().acknowledge().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), progress.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*progress.borrow(), 1);
    server_task.await.unwrap();
    stop(opened).await;
}

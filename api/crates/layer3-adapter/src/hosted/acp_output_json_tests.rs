use super::*;

fn bounded_settings() -> HostedAgentOpenSettings {
    let mut settings = settings(None, false);
    settings.output_json_budget = true;
    settings
}

fn amplified() -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0", "method":"unused", "params":vec![0; 8192]})
}

async fn raw_output(server: &mut ServerSocket, cursor: u64, payload: &[u8]) {
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend_from_slice(stream().as_bytes());
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend_from_slice(payload);
    server.send(Message::Binary(bytes)).await.unwrap();
}

async fn assert_no_ack(server: &mut ServerSocket) {
    while let Ok(Some(Ok(message))) =
        tokio::time::timeout(Duration::from_millis(50), server.next()).await
    {
        if let Message::Binary(bytes) = message {
            assert_ne!(
                bytes.first(),
                Some(&LANE_OUTPUT_ACK),
                "rejected output was ACKed"
            );
        }
    }
}

#[tokio::test]
async fn enabled_policy_refuses_legacy_ready_before_initialization() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (updates, mut received) = mpsc::channel(4);
    let result = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        bounded_settings(),
        None,
        updates,
    )
    .await;
    assert!(
        matches!(result, Err(AdapterError::Rejected(message)) if message.contains("requires retained"))
    );
    assert!(received.recv().await.is_none());
    if let Ok(Some(Ok(Message::Binary(_)))) =
        tokio::time::timeout(Duration::from_millis(50), server.next()).await
    {
        panic!("refused Ready must not initialize or submit a prompt");
    }
}

#[test]
fn ready_is_preflighted_before_its_owned_fields_are_decoded() {
    let mut lanes = vec!["ctl", "conv"];
    lanes.extend(std::iter::repeat_n("other", 8192));
    let ready = serde_json::json!({"t":"ready", "wireVersion":1, "lanes":lanes,
        "agentId":"agent-1", "outputAcknowledgement":{"version":1,"streamId":stream()}});
    assert!(validate_ready(Message::Text(ready.to_string()), "agent-1", false).is_ok());
    assert!(
        matches!(validate_ready(Message::Text(ready.to_string()), "agent-1", true),
        Err(AdapterError::Rejected(message)) if message.contains("node limit"))
    );
}

#[tokio::test]
async fn rejected_bootstrap_output_never_publishes_a_receipt_or_advances_handshake() {
    for payload in [serde_json::to_vec(&amplified()).unwrap(), b"{".to_vec()] {
        let (client, mut server) = socket_pair().await;
        reliable_ready(&mut server).await;
        let server_task = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            assert_eq!(initialize["method"], "initialize");
            raw_output(&mut server, 1, &payload).await;
            // Rejection cannot send a cursor ACK or advance to session/new.
            if let Ok(Some(Ok(Message::Binary(bytes)))) =
                tokio::time::timeout(Duration::from_millis(100), server.next()).await
            {
                panic!(
                    "rejected bootstrap emitted a frame on lane {:?}",
                    bytes.first()
                );
            }
        });
        let (updates, mut received) = mpsc::channel(8);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            Bridge::open(
                client,
                "agent-1",
                "/workspace".into(),
                bounded_settings(),
                None,
                updates,
            ),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(update) = received.recv().await {
                assert!(!matches!(update, ConversationUpdate::RuntimeOutput { .. }));
            }
        })
        .await
        .expect("failed startup releases its update sender");
        server_task.await.unwrap();
    }
}

async fn active_policy(enabled: bool, malformed: bool) {
    let prompt = crate::PromptIdentity::new("conversation-budget", 1, 1);
    let expected = prompt.clone();
    let (client, mut server) = socket_pair().await;
    reliable_ready(&mut server).await;
    let (after_terminal, terminal_seen) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        retained(
            &mut server,
            1,
            serde_json::json!({"jsonrpc":"2.0", "id":initialize["id"], "result":{}}),
        )
        .await;
        let open = next_conversation(&mut server).await;
        assert_eq!(open["method"], "session/new");
        retained(
            &mut server,
            2,
            serde_json::json!({"jsonrpc":"2.0", "id":open["id"],
            "result":{"sessionId":"conversation-budget"}}),
        )
        .await;
        retained(
            &mut server,
            3,
            serde_json::json!({"jsonrpc":"2.0", "id":expected.as_str(),
            "result":{"stopReason":"end_turn"}}),
        )
        .await;
        // Valid output is only acknowledged after the explicit consumer commit.
        let message = tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Binary(ack) = message else {
            panic!("expected durable ACK")
        };
        assert_eq!(ack[0], LANE_OUTPUT_ACK);
        assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), 3);
        after_terminal.send(()).unwrap();
        if malformed {
            raw_output(&mut server, 4, b"{").await;
        } else {
            retained(&mut server, 4, amplified()).await;
        }
        if enabled {
            assert_no_ack(&mut server).await;
        } else {
            let message = tokio::time::timeout(Duration::from_secs(2), server.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let Message::Binary(ack) = message else {
                panic!("expected legacy ACK")
            };
            assert_eq!(ack[0], LANE_OUTPUT_ACK);
            assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), 4);
        }
    });
    let (updates, mut received) = mpsc::channel(8);
    let mut settings = settings(None, false);
    settings.output_json_budget = enabled;
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings,
        None,
        updates,
    )
    .await
    .unwrap();
    for cursor in 1..=3 {
        let next = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, update } = next else {
            panic!("retained receipt missing")
        };
        assert_eq!(receipt.source.cursor, cursor);
        if cursor == 3 {
            assert!(
                matches!(update.as_deref(), Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }) if prompt_id == &prompt)
            );
            receipt.acknowledge().await.unwrap();
        }
    }
    terminal_seen.await.unwrap();
    if enabled {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !opened.reader.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        while let Ok(update) = received.try_recv() {
            assert!(
                !matches!(update, ConversationUpdate::RuntimeOutput { receipt, .. } if receipt.source.cursor == 4)
            );
        }
    } else {
        let update = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, update } = update else {
            panic!("legacy receipt missing")
        };
        assert_eq!(receipt.source.cursor, 4);
        assert!(update.is_none());
        receipt.acknowledge().await.unwrap();
    }
    server_task.await.unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn enabled_active_output_preserves_valid_terminal_and_rejects_excess_nodes_without_ack() {
    active_policy(true, false).await;
}

#[tokio::test]
async fn enabled_active_output_rejects_malformed_json_without_ack() {
    active_policy(true, true).await;
}

#[tokio::test]
async fn disabled_output_preserves_legacy_receipts_for_unrecognized_json() {
    active_policy(false, false).await;
    active_policy(false, true).await;
}

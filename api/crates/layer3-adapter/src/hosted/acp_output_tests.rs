use super::*;

fn stream() -> uuid::Uuid {
    uuid::Uuid::parse_str("d99003d1-0c3c-46cd-aa47-9bbdbed50b12").unwrap()
}

async fn reliable_ready(server: &mut ServerSocket) {
    server
        .send(Message::Text(
            serde_json::json!({
                "t": "ready", "wireVersion": 1, "lanes": ["ctl", "conv"], "agentId": "agent-1",
                "outputAcknowledgement": {"version": 1, "streamId": stream()},
            })
            .to_string(),
        ))
        .await
        .unwrap();
}

async fn retained(server: &mut ServerSocket, cursor: u64, value: serde_json::Value) {
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend_from_slice(stream().as_bytes());
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend(serde_json::to_vec(&value).unwrap());
    server.send(Message::Binary(bytes)).await.unwrap();
}

#[tokio::test]
async fn retained_backlog_over_256_keeps_terminal_and_receipts_ordered_through_handshake() {
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
            "jsonrpc": "2.0", "id": initialize["id"], "error": {"code": -32000, "message": "already initialized"},
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
    let opening = tokio::spawn(Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(prompt),
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
        assert_eq!(receipt.source.native_conversation_id, "conversation-held");
        if cursor == 301 {
            assert!(matches!(
                update.as_deref(),
                Some(ConversationUpdate::TurnEnded(_))
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
    first_receipt.unwrap().acknowledge().await.unwrap();
    server_task.await.unwrap();
    stop(opened).await;
}

#[test]
fn output_capability_is_optional_and_rejects_unknown_versions() {
    for version in [0, 2] {
        let ready = serde_json::json!({
            "t": "ready", "wireVersion": 1, "lanes": ["ctl", "conv"], "agentId": "agent-1",
            "outputAcknowledgement": {"version": version, "streamId": stream()},
        });
        assert!(validate_ready(Message::Text(ready.to_string()), "agent-1").is_err());
    }
    let ready = serde_json::json!({"t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1"});
    assert_eq!(
        validate_ready(Message::Text(ready.to_string()), "agent-1").unwrap(),
        None
    );
}

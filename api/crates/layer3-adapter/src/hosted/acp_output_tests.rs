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
    let expected_id = crate::PromptIdentity::new("conversation-held", 19, 158)
        .as_str()
        .to_owned();
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
                Some(ConversationUpdate::TurnEndedForPrompt { .. })
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

#[tokio::test]
async fn quiet_session_load_drains_over_eight_mib_and_16384_frames_before_reply() {
    let (client, mut server) = socket_pair().await;
    reliable_ready(&mut server).await;
    let (finish, finished) = tokio::sync::oneshot::channel();
    let count = 16_385_u64;
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        retained(
            &mut server,
            1,
            serde_json::json!({
                "jsonrpc":"2.0", "id":initialize["id"], "result":{"protocolVersion":1},
            }),
        )
        .await;
        // Initialize's durable ACK and session/load can arrive in either order.
        let mut load = None;
        let mut acknowledged_through = 0;
        while load.is_none() {
            let Message::Binary(frame) = server.next().await.unwrap().unwrap() else {
                panic!("binary frame required")
            };
            if frame[0] == LANE_OUTPUT_ACK {
                acknowledged_through = u64::from_be_bytes(frame[17..25].try_into().unwrap());
            } else {
                let value: serde_json::Value = serde_json::from_slice(&frame[1..]).unwrap();
                assert_eq!(value["method"], "session/load");
                assert_eq!(value["params"]["sessionId"], "conversation-held");
                load = Some(value);
            }
        }
        let mut bytes = 0;
        for cursor in 2..=count + 1 {
            let value = serde_json::json!({"jsonrpc":"2.0", "method":"session/update", "params":{
                "sessionId":"conversation-held", "update":{
                    "sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":format!("{cursor}:{}", "x".repeat(1024))}
                }
            }});
            bytes += serde_json::to_vec(&value).unwrap().len();
            retained(&mut server, cursor, value).await;
        }
        // Load's reply follows a burst larger than both real retention limits.
        // The old adapter overflows its startup queue before this can complete.
        while acknowledged_through < count + 1 {
            let Message::Binary(ack) = server.next().await.unwrap().unwrap() else {
                panic!("ACK required")
            };
            assert_eq!(ack[0], LANE_OUTPUT_ACK);
            let cursor = u64::from_be_bytes(ack[17..25].try_into().unwrap());
            assert_eq!(cursor, acknowledged_through + 1);
            acknowledged_through = cursor;
        }
        assert!(bytes > 8 * 1024 * 1024);
        retained(
            &mut server,
            count + 2,
            serde_json::json!({
                "jsonrpc":"2.0", "id":load.unwrap()["id"], "result":{},
            }),
        )
        .await;
        finished.await.unwrap();
    });
    let (updates, mut receiver) = mpsc::channel(4);
    let opening = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some("conversation-held".into()),
        updates,
    );
    tokio::pin!(opening);
    let mut last = 0;
    let opened = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            tokio::select! {
                result = &mut opening => break result.unwrap(),
                update = receiver.recv() => {
                    let ConversationUpdate::RuntimeOutput { receipt, update } = update.unwrap() else { panic!("receipt required") };
                    last += 1;
                    assert_eq!(receipt.source.cursor, last);
                    assert_eq!(receipt.source.native_conversation_id, "conversation-held");
                    if (2..=count + 1).contains(&last) {
                        assert!(matches!(update.as_deref(), Some(ConversationUpdate::WithoutPromptEvidence(_))));
                    }
                    // CMA tests exercise the durable transaction owning this
                    // ACK; parsing alone never emits an acknowledgement.
                    receipt.acknowledge().await.unwrap();
                }
            }
        }
    }).await.expect("startup replay must make progress before load completes");
    assert!(last > count);
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-held")
    );
    finish.send(()).unwrap();
    server_task.await.unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn startup_idle_deadline_advances_only_after_durable_ack_progress() {
    for acknowledge in [true, false] {
        let (client, mut server) = socket_pair().await;
        reliable_ready(&mut server).await;
        let (finish, finished) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            retained(&mut server, 1, serde_json::json!({"jsonrpc":"2.0", "id":initialize["id"], "error":{
                "code":-32603,"message":"Internal error","data":{"details":"Already initialized"}
            }})).await;
            let load = loop {
                let Message::Binary(frame) = server.next().await.unwrap().unwrap() else {
                    panic!("binary frame")
                };
                if frame[0] == LANE_CONVERSATION {
                    break serde_json::from_slice::<serde_json::Value>(&frame[1..]).unwrap();
                }
            };
            for cursor in 2..=6 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                retained(
                    &mut server,
                    cursor,
                    serde_json::json!({"jsonrpc":"2.0","method":"cma/noop"}),
                )
                .await;
            }
            retained(
                &mut server,
                7,
                serde_json::json!({"jsonrpc":"2.0", "id":load["id"], "result":{}}),
            )
            .await;
            finished.await.unwrap();
        });
        let (send, mut received) = mpsc::channel(8);
        let opened = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            settings(None, false),
            Some("conversation-held".into()),
            send,
        )
        .await
        .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, .. } = received.recv().await.unwrap()
        else {
            panic!("receipt")
        };
        receipt.acknowledge().await.unwrap();
        let load = opened.bridge.call_within(
            "session/load",
            serde_json::json!({"sessionId":"conversation-held"}),
            Duration::from_millis(120),
        );
        tokio::pin!(load);
        let result = loop {
            tokio::select! {
                result = &mut load => break result,
                update = received.recv() => {
                    let ConversationUpdate::RuntimeOutput { receipt, .. } = update.unwrap() else { panic!("receipt") };
                    if acknowledge { receipt.acknowledge().await.unwrap(); }
                }
            }
        };
        assert_eq!(result.is_ok(), acknowledge);
        finish.send(()).unwrap();
        server_task.await.unwrap();
        // load borrows the bridge until this scope ends.
        opened.bridge.retire().await;
        opened.reader.abort();
        opened.relay.abort();
    }
}

#[tokio::test]
async fn cancelling_open_closes_its_reader_and_relay_before_retained_output_can_stall() {
    let (client, mut server) = socket_pair().await;
    reliable_ready(&mut server).await;
    let (updates, mut received) = mpsc::channel(1);
    let opening = tokio::spawn(Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some("conversation-held".into()),
        updates,
    ));
    let _initialize = receive_json(&mut server).await;
    opening.abort();
    assert!(opening.await.err().unwrap().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .is_none()
    );
    let ended = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap();
    assert!(matches!(
        ended,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
}

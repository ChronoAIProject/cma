use super::*;

#[tokio::test]
async fn owned_terminal_interleaving_preserves_receipts_and_rejects_late_ack_for_three_epochs() {
    for epoch in 1..=3 {
        retained_backlog_with_owner(Some(
            crate::owner_protocol::test_support::TestAuthority::new(epoch),
        ))
        .await;
    }
}

async fn retained_backlog_with_owner(
    authority: Option<std::sync::Arc<crate::owner_protocol::test_support::TestAuthority>>,
) {
    use crate::owner_protocol::{scope, TransportAuthority};
    let prompt = crate::PromptIdentity::new("conversation-held", 19, 157);
    let expected_id = prompt.as_str().to_owned();
    let expected_prompt = prompt.clone();
    let (client, mut server) = socket_pair().await;
    if let Some(authority) = &authority {
        server
            .send(Message::Text(
                serde_json::json!({
                    "t":"ready", "wireVersion":1,"lanes":["ctl","conv"],"agentId":"agent-1",
                    "outputAcknowledgement":{"version":1,"streamId":stream()},
                    "managerOwnership":{"protocol":1,"owner":authority.owner()},
                })
                .to_string(),
            ))
            .await
            .unwrap();
    } else {
        reliable_ready(&mut server).await;
    }
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
    let opening_future = Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(prompt),
        updates,
    );
    let opening_owner = authority.clone();
    let opening = tokio::spawn(async move {
        match opening_owner {
            Some(owner) => scope(owner, opening_future).await,
            None => opening_future.await,
        }
    });
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
                Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }) if prompt_id == &expected_prompt
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
    let first_receipt = first_receipt.unwrap();
    let source = first_receipt.source.clone();
    first_receipt.acknowledge().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), progress.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*progress.borrow(), 1);
    if let Some(authority) = authority {
        let ack_state = opened.bridge.output_ack_state().await;
        assert!(ack_state.is_some());
        authority.revoke();
        assert!(opened
            .bridge
            .acknowledge_output(source, ack_state.as_ref())
            .await
            .is_err());
    }
    server_task.await.unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn owned_ready_requires_exact_epoch_and_durable_output_without_legacy_downgrade() {
    use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};
    let authority = TestAuthority::new(2);
    let base = serde_json::json!({
        "t":"ready", "wireVersion":1,"lanes":["ctl","conv"],"agentId":"agent-1",
        "outputAcknowledgement":{"version":1,"streamId":stream()},
        "managerOwnership":{"protocol":1,"owner":authority.owner()},
    });
    assert!(scope(authority.clone(), async {
        validate_ready(Message::Text(base.to_string()), "agent-1", false, None)
    })
    .await
    .is_ok());
    for defect in ["missing", "epoch", "protocol", "ack"] {
        let mut ready = base.clone();
        match defect {
            "missing" => {
                ready.as_object_mut().unwrap().remove("managerOwnership");
            }
            "epoch" => ready["managerOwnership"]["owner"]["epoch"] = 1.into(),
            "protocol" => ready["managerOwnership"]["protocol"] = 2.into(),
            _ => {
                ready
                    .as_object_mut()
                    .unwrap()
                    .remove("outputAcknowledgement");
            }
        }
        assert!(scope(authority.clone(), async {
            validate_ready(Message::Text(ready.to_string()), "agent-1", false, None)
        })
        .await
        .is_err());
    }
    assert!(validate_ready(Message::Text(base.to_string()), "agent-1", false, None).is_err());
}

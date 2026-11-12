use super::*;

fn terminal(id: &PromptIdentity, failure: bool) -> serde_json::Value {
    if failure {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id.as_str(),
            "error": {"code": -32000, "message": "expected buffered failure"},
        })
    } else {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id.as_str(),
            "result": {"stopReason": "end_turn"},
        })
    }
}

#[tokio::test]
async fn buffered_success_keeps_captured_identity_after_foreign_terminal() {
    buffered_terminal_keeps_identity_after_foreign_terminal(false).await;
}

#[tokio::test]
async fn buffered_failure_keeps_captured_identity_after_delayed_terminal() {
    buffered_terminal_keeps_identity_after_foreign_terminal(true).await;
}

async fn buffered_terminal_keeps_identity_after_foreign_terminal(expected_failure: bool) {
    let expected = PromptIdentity::new("conversation-held", 17, 8);
    let foreign = PromptIdentity::new("conversation-held", 17, 7);
    let expected_for_server = expected.clone();
    let foreign_for_server = foreign.clone();
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (sent, observed) = tokio::sync::oneshot::channel();
    let (verify, verified) = tokio::sync::oneshot::channel();
    let (release, held) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        send_json(
            &mut server,
            terminal(&expected_for_server, expected_failure),
        )
        .await;
        send_json(&mut server, terminal(&foreign_for_server, false)).await;
        sent.send(()).unwrap();
        verified.await.unwrap();
        reject(&mut server, &initialize).await;
        tokio::select! {
            _ = held => {}
            unexpected = server.next() => panic!("recovery sent native input: {unexpected:?}"),
        }
    });
    let (updates, mut received) = mpsc::channel(8);
    let opening = tokio::spawn(Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(expected.clone()),
        updates,
    ));
    observed.await.unwrap();

    let foreign_update = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .expect("foreign terminal arrives before handshake")
        .expect("update relay remains open");
    assert!(matches!(
        foreign_update,
        ConversationUpdate::TurnEndedForPrompt { prompt_id, .. } if prompt_id == foreign
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(40), received.recv())
            .await
            .is_err()
    );

    verify.send(()).unwrap();
    let opened = opening.await.unwrap().expect("retained session verifies");
    let recovered = tokio::time::timeout(Duration::from_secs(1), received.recv())
        .await
        .expect("buffered terminal releases after handshake")
        .expect("update relay remains open");
    if expected_failure {
        assert!(matches!(
            recovered,
            ConversationUpdate::FailedForPrompt { prompt_id, message, .. }
                if prompt_id == expected && message == "expected buffered failure"
        ));
    } else {
        assert!(matches!(
            recovered,
            ConversationUpdate::TurnEndedForPrompt { prompt_id, .. } if prompt_id == expected
        ));
    }
    release.send(()).unwrap();
    server_task.await.unwrap();
    stop(opened).await;
}

async fn reliable_ready(server: &mut ServerSocket) {
    server
        .send(Message::Text(
            serde_json::json!({
                "t": "ready",
                "wireVersion": 1,
                "lanes": ["ctl", "conv"],
                "agentId": "agent-1",
                "outputAcknowledgement": {
                    "version": 1,
                    "streamId": "d99003d1-0c3c-46cd-aa47-9bbdbed50b12"
                },
            })
            .to_string(),
        ))
        .await
        .unwrap();
}

async fn retained(server: &mut ServerSocket, cursor: u64, value: serde_json::Value) {
    let mut frame = vec![LANE_RETAINED_OUTPUT];
    frame.extend_from_slice(
        uuid::Uuid::parse_str("d99003d1-0c3c-46cd-aa47-9bbdbed50b12")
            .unwrap()
            .as_bytes(),
    );
    frame.extend_from_slice(&cursor.to_be_bytes());
    frame.extend(serde_json::to_vec(&value).unwrap());
    server.send(Message::Binary(frame)).await.unwrap();
}

#[tokio::test]
async fn reliable_output_replays_foreign_and_recovered_terminals_with_their_identities() {
    let expected = PromptIdentity::new("conversation-held", 17, 8);
    let foreign = PromptIdentity::new("conversation-held", 17, 7);
    let expected_for_server = expected.clone();
    let foreign_for_server = foreign.clone();
    let (client, mut server) = socket_pair().await;
    reliable_ready(&mut server).await;
    let (release, held) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        retained(&mut server, 1, terminal(&expected_for_server, false)).await;
        retained(&mut server, 2, terminal(&foreign_for_server, false)).await;
        retained(
            &mut server,
            3,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": initialize["id"].clone(),
                "error": {
                    "code": -32603,
                    "message": "Internal error",
                    "data": {"details": "Already initialized"},
                },
            }),
        )
        .await;
        held.await.unwrap();
    });
    let (updates, mut received) = mpsc::channel(8);
    let opened = Bridge::open_with_subscription_and_prompt(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None),
        Some("conversation-held".into()),
        Some(expected.clone()),
        updates,
    )
    .await
    .expect("retained session verifies");

    let mut terminals = Vec::new();
    for cursor in 1..=3 {
        let update = tokio::time::timeout(Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, update } = update else {
            panic!("reliable output must preserve every receipt")
        };
        assert_eq!(receipt.source.cursor, cursor);
        if let Some(update) = update {
            if let ConversationUpdate::TurnEndedForPrompt { prompt_id, .. } = &*update {
                terminals.push(prompt_id.clone());
            }
        }
        receipt.acknowledge().await.unwrap();
    }
    assert_eq!(terminals, vec![expected, foreign]);
    release.send(()).unwrap();
    server_task.await.unwrap();
    stop(opened).await;
}

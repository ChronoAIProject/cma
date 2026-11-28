use super::super::tests::{receive_json, settings, socket_pair, stop, ServerSocket};
use super::super::LANE_RETAINED_OUTPUT;
use super::budget_tests::{open_with_json, retained};
use super::*;
use futures_util::StreamExt as _;
use serde_json::{json, Value};
use std::time::Duration;

async fn answer(server: &mut ServerSocket, cursor: u64, request: Value, result: Value) {
    let payload = serde_json::to_vec(&json!({"id":request["id"],"result":result})).unwrap();
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend(retained(cursor, &payload));
    server.send(Message::Binary(bytes)).await.unwrap();
}

#[tokio::test]
async fn retained_startup_preserves_exact_identity_or_refuses_without_effect_or_ack() {
    for bounded in [false, true] {
        for (identity, accepted) in [
            (json!("é".repeat(512)), true),
            (json!("x".repeat(1024)), true),
            (json!("x".repeat(1025)), false),
            (json!(""), false),
            (json!("bad\nidentity"), false),
            (Value::Null, false),
            (json!({"private":"value"}), false),
        ] {
            let expected = accepted.then(|| identity.as_str().unwrap().to_owned());
            let (client, mut server) = socket_pair().await;
            server
                .send(Message::Text(
                    json!({
                        "t":"ready","wireVersion":1,"lanes":["ctl","conv"],"agentId":"agent-1",
                        "outputAcknowledgement":{"version":1,"streamId":uuid::Uuid::from_u128(1710)}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            let mut config = settings(None, false);
            config.retained_json = bounded;
            config.bounded_raw_output = true;
            config.output_json_budget = true;
            let (sender, mut received) = mpsc::channel(8);
            let opening = tokio::spawn(Bridge::open_with_relay_policy(
                client,
                "agent-1",
                "/workspace".into(),
                (config, None, (false, None)),
                None,
                None,
                sender,
            ));
            let initialize = receive_json(&mut server).await;
            answer(&mut server, 1, initialize, json!({})).await;
            let request = receive_json(&mut server).await;
            assert_eq!(request["method"], "session/new");
            answer(&mut server, 2, request, json!({"sessionId":identity})).await;
            let outcome = tokio::time::timeout(Duration::from_secs(2), opening)
                .await
                .unwrap()
                .unwrap();
            if let Some(expected) = expected {
                let opened = outcome.unwrap();
                assert_eq!(opened.bridge.session().await.unwrap(), expected);
                assert_eq!(
                    opened
                        .bridge
                        .output
                        .as_ref()
                        .unwrap()
                        .lock()
                        .await
                        .session
                        .as_deref(),
                    Some(expected.as_str())
                );
                for cursor in 1..=2 {
                    let update = tokio::time::timeout(Duration::from_secs(2), received.recv())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(
                        matches!(update, ConversationUpdate::RuntimeOutput {receipt, update:None}
                        if receipt.source.cursor==cursor && receipt.source.native_conversation_id==expected)
                    );
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(25), server.next())
                        .await
                        .is_err(),
                    "receipts require explicit durable acceptance"
                );
                stop(opened).await;
            } else {
                let expected_error = if identity.is_string() {
                    "bridge returned an invalid conversation identifier"
                } else {
                    "bridge named no conversation"
                };
                assert!(
                    matches!(outcome, Err(AdapterError::Rejected(message)) if message==expected_error)
                );
                assert!(
                    received.try_recv().is_err(),
                    "refusal cannot publish startup effects or receipts"
                );
                if let Ok(Some(Ok(message))) =
                    tokio::time::timeout(Duration::from_millis(25), server.next()).await
                {
                    assert!(
                        !matches!(message, Message::Binary(_)),
                        "refusal cannot ACK or send another protocol effect"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn common_activation_guard_still_refuses_invalid_and_changed_identities() {
    for bounded in [false, true] {
        let (opened, _server, _received) = open_with_json(true, false, bounded).await;
        let (capture, mut effects) = UpdateRelay::capture();
        for invalid in [
            "".into(),
            "x".repeat(1025),
            "bad\nidentity".into(),
            "different".into(),
        ] {
            assert!(matches!(
                opened.bridge.activate_session(invalid, &capture).await,
                Err(AdapterError::Rejected(_))
            ));
            assert_eq!(opened.bridge.session().await.unwrap(), "conversation-1");
            assert!(effects.try_recv().is_err());
        }
        stop(opened).await;
    }
}

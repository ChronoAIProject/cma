use super::super::tests::{receive_json, stop};
use super::budget_tests::{open_with_json, retained};
use super::*;
use futures_util::StreamExt as _;
use serde_json::{json, Value};
use std::time::Duration;

fn permission(call: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id":call,"method":"session/request_permission",
        "params":{"sessionId":"conversation-1","toolCall":{"toolCallId":"tool","title":"Permission"},
        "options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}})).unwrap()
}

fn is_permission(update: &ConversationUpdate) -> bool {
    match update {
        ConversationUpdate::WithoutPromptEvidence(update) => is_permission(update),
        ConversationUpdate::Asked(request) => request.request.options.len() == 1,
        _ => false,
    }
}

#[tokio::test]
async fn retained_permission_ids_preserve_allowed_effects_and_refuse_invalid_ids_without_ack() {
    for bounded in [false, true] {
        let (opened, mut server, _received) = open_with_json(true, true, bounded).await;
        let (capture, mut effects) = UpdateRelay::capture();
        for (offset, (call, accepted)) in [
            (json!("x".repeat(254)), true),
            (json!("\n".repeat(127)), true),
            (json!(42), true),
            (json!("x".repeat(255)), false),
            (json!("x".repeat(1024 * 1024)), false),
            (json!(["nested"]), false),
            (Value::Null, false),
        ]
        .into_iter()
        .enumerate()
        {
            let cursor = 3 + offset as u64;
            opened
                .bridge
                .receive_output(&retained(cursor, &permission(&call)), &capture)
                .await
                .unwrap();
            let ConversationUpdate::RuntimeOutput { receipt, update } = effects.try_recv().unwrap()
            else {
                panic!("source receipt")
            };
            assert_eq!(receipt.source.cursor, cursor);
            if accepted {
                let update = update.unwrap();
                assert!(is_permission(&update));
                assert!(
                    update.turn_evidence().is_none(),
                    "passive observation cannot establish prompt authority"
                );
            } else {
                assert!(update.is_none());
            }
            drop(receipt);
            assert!(effects.try_recv().is_err());
            assert!(
                tokio::time::timeout(Duration::from_millis(25), server.next())
                    .await
                    .is_err()
            );
        }
        stop(opened).await;
    }
}

#[tokio::test]
async fn subscription_replies_keep_exact_allowed_ids_and_emit_nothing_for_invalid_ids() {
    for bounded in [false, true] {
        let (opened, mut server, _received) = open_with_json(true, false, bounded).await;
        let (capture, mut effects) = UpdateRelay::capture();
        for (offset, (call, accepted)) in [
            (json!("é".repeat(127)), true),
            (json!(17), true),
            (json!("é".repeat(128)), false),
            (json!("x".repeat(1024 * 1024)), false),
            (json!({"id":"nested"}), false),
        ]
        .into_iter()
        .enumerate()
        {
            let payload = serde_json::to_vec(
                &json!({"id":call,"method":"cma/codex/credentials","params":{}}),
            )
            .unwrap();
            opened
                .bridge
                .receive_output(&retained(3 + offset as u64, &payload), &capture)
                .await
                .unwrap();
            assert!(matches!(
                effects.try_recv().unwrap(),
                ConversationUpdate::RuntimeOutput { update: None, .. }
            ));
            if accepted {
                let reply = receive_json(&mut server).await;
                assert_eq!(reply["id"], call);
                assert_eq!(reply["error"]["code"], -32001);
                assert!(reply.get("result").is_none());
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(25), server.next())
                    .await
                    .is_err(),
                "invalid IDs get no reply; no dropped receipt is an ACK"
            );
            assert!(effects.try_recv().is_err());
        }
        stop(opened).await;
    }
}

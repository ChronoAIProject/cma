#[tokio::test]
async fn independent_legacy_flags_preserve_rpc_errors_receipts_and_ack_order() {
    use super::super::{tests as legacy, Bridge};
    use crate::hosted::process_ingress_test_support::{
        process_budget_lock, wait_for_process_budget,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    let _process_budget = process_budget_lock().await;
    let before = crate::ingress_budget::IngressBudget::process().available_bytes();
    let mut combinations = 0;
    for flags in 0..128 {
        if flags & 1 != 0 && flags & 14 != 14 {
            continue;
        }
        let (client, mut server) = legacy::socket_pair().await;
        let mut settings = legacy::settings(None, false);
        settings.coordinated_ingress = flags & 1 != 0;
        settings.retained_json = flags & 2 != 0;
        settings.bounded_raw_output = flags & 4 != 0;
        settings.bounded_permission_state = flags & 8 != 0;
        settings.output_json_budget = flags & 16 != 0;
        settings.shared_runtime_output = flags & 32 != 0;
        let (updates, mut received) = tokio::sync::mpsc::channel(8);
        let peer =
            async move {
                server.send(super::Message::Text(json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent",
                "outputAcknowledgement":{"version":1, "streamId":uuid::Uuid::from_u128(1)}
            }).to_string())).await.unwrap();
                for (cursor, result) in [
                    (1, json!({"protocolVersion":1, "agentCapabilities":{}})),
                    (2, json!({"sessionId":"conversation"})),
                ] {
                    let request = legacy::receive_json(&mut server).await;
                    send_legacy_retained(
                        &mut server,
                        cursor,
                        json!({"id":request["id"], "result":result}),
                    )
                    .await;
                }
                server
            };
        let (opened, mut server) = tokio::join!(
            Bridge::open_with_relay_policy(
                client,
                "agent",
                "/synthetic/work".into(),
                (settings, None, (flags & 64 != 0, None)),
                None,
                None,
                updates,
            ),
            peer
        );
        let opened = opened.unwrap();
        assert!(opened.bridge.ingress.is_none(), "flags={flags}");
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("conversation")
        );
        let bridge = opened.bridge.clone();
        let call = tokio::spawn(async move { bridge.call("fixture/error", json!({})).await });
        let request = legacy::receive_json(&mut server).await;
        send_legacy_retained(
            &mut server,
            3,
            json!({"id":request["id"], "error":{
                "code":-32000, "message":"synthetic refusal"
            }}),
        )
        .await;
        assert!(
            matches!(call.await.unwrap(), Err(crate::AdapterError::Rejected(message))
            if message == "fixture/error: {\"code\":-32000,\"message\":\"synthetic refusal\"}")
        );
        for cursor in 1..=3 {
            let update = received.recv().await.unwrap();
            let delivery = update.delivery();
            let receipt = delivery.receipt.unwrap();
            assert_eq!(receipt.source.cursor, cursor);
            assert_eq!(receipt.source.native_conversation_id, "conversation");
            assert!(delivery.effect.is_none());
            receipt.acknowledge().await.unwrap();
            let super::Message::Binary(ack) = server.next().await.unwrap().unwrap() else {
                panic!("missing ACK")
            };
            assert_eq!(ack[0], 5);
            assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), cursor);
        }
        legacy::stop(opened).await;
        drop((server, received));
        wait_for_process_budget(before).await;
        combinations += 1;
    }
    assert_eq!(combinations, 72);
}

async fn send_legacy_retained(
    server: &mut super::super::tests::ServerSocket,
    cursor: u64,
    value: serde_json::Value,
) {
    use futures_util::SinkExt;
    let mut wire = vec![4];
    wire.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
    wire.extend_from_slice(&cursor.to_be_bytes());
    wire.extend_from_slice(&serde_json::to_vec(&value).unwrap());
    server.send(super::Message::Binary(wire)).await.unwrap();
}

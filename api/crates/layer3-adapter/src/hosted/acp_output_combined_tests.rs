use super::*;

fn permission(option: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0", "id":1, "method":"session/request_permission",
        "params":{"sessionId":"conversation-1", "toolCall":{"toolCallId":"tool", "title":"Permission"},
            "options":[{"optionId":option,"name":"Allow", "kind":"allow_once"}]}
    }))
    .unwrap()
}

#[tokio::test]
async fn combined_policies_admit_once_after_raw_refusal_and_preserve_exact_ack() {
    let (opened, mut server, _received) = open_with_permissions(true, true).await;
    let pool = budget(&opened, 8192, false).await;
    let held = pool.clone().try_acquire_many_owned(8192).unwrap();
    let (capture, mut updates) = UpdateRelay::capture();
    let payload = permission("allow");
    let frame = retained(3, &payload);
    assert!(matches!(
        opened.bridge.receive_output(&frame, &capture).await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert!(updates.try_recv().is_err());
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(2)
    );
    drop(held);
    opened
        .bridge
        .receive_output(&frame, &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, update } = updates.try_recv().unwrap() else {
        panic!("accepted permission requires its original source receipt")
    };
    let permission = match *update.unwrap() {
        ConversationUpdate::WithoutPromptEvidence(update) => *update,
        other => other,
    };
    assert!(matches!(permission, ConversationUpdate::Asked(_)));
    assert!(updates.try_recv().is_err());
    assert_eq!(pool.available_permits(), 8192);
    assert_eq!(receipt.source.cursor, 3);
    assert_eq!(
        receipt.source.payload_sha256,
        format!("{:x}", Sha256::digest(payload))
    );
    receipt.acknowledge().await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(ack) = ack else {
        panic!("source ACK")
    };
    assert_eq!(ack[0], LANE_OUTPUT_ACK);
    assert_eq!(&ack[1..17], stream().as_bytes());
    assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), 3);
    assert!(opened.bridge.blocked.lock().await.is_empty());
    assert!(opened.bridge.answered.lock().await.is_empty());
    stop(opened).await;
}

#[tokio::test]
async fn combined_permission_refusal_releases_raw_storage_without_receipt_or_terminal() {
    let (mut opened, mut server, mut received) = open_with_permissions(true, true).await;
    let pool = budget(&opened, 8192, false).await;
    let (capture, mut prompt_effects) = UpdateRelay::capture();
    opened
        .bridge
        .drive(ConversationInput::Prompt(vec![]), &capture)
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await["method"], "session/prompt");
    let mut frame = vec![LANE_RETAINED_OUTPUT];
    frame.extend(retained(3, &permission(&"x".repeat(257))));
    server.send(Message::Binary(frame)).await.unwrap();
    assert_eq!(receive_json(&mut server).await["error"]["code"], -32002);
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pool.available_permits(), 8192);
    assert!(received.try_recv().is_err());
    assert_eq!(
        *opened
            .bridge
            .managed_context_progress()
            .await
            .unwrap()
            .borrow(),
        0
    );
    drop(capture);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), prompt_effects.recv())
            .await
            .unwrap()
            .is_none()
    );
    opened.bridge.retire().await;
    opened.relay.abort();
    let _ = opened.relay.await;
}

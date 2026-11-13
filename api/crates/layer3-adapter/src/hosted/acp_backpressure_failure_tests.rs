use super::*;

#[tokio::test]
async fn cancelling_reader_cancels_its_writer_without_acknowledging_uncommitted_output() {
    let gate = Arc::new(FlushGate::default());
    let (mut opened, mut received, mut server) = opened_pair_with_gate(Some(gate.clone())).await;
    let progress = opened.bridge.output_progress().await.unwrap();
    retained(&mut server, 3, chunk()).await;
    let (receipt, _) = next_update(&mut received).await;
    gate.blocked.store(true, Ordering::SeqCst);
    retained(
        &mut server,
        4,
        serde_json::json!({"jsonrpc":"2.0", "id":987, "method":"unsupported/fixture", "params":{}}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
        .await
        .unwrap();
    receipt.acknowledge().await.unwrap();
    opened.reader.abort();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    opened.relay.abort();
    let _ = opened.relay.await;
    drop(opened.channel);
    // Receipt 3 was committed above, so its queued ACK may race cancellation.
    // Frame 4 was never committed. Its output must remain unacknowledged, and
    // no object outside the cancelled reader may retain the ACK writer.
    drop(opened.bridge);
    drop(received);
    drop(receipt);
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(message)) = server.next().await {
            if let Message::Binary(bytes) = message {
                if bytes.first() == Some(&LANE_OUTPUT_ACK) {
                    assert_eq!(u64::from_be_bytes(bytes[17..25].try_into().unwrap()), 3);
                }
            }
        }
    })
    .await
    .expect("reader cancellation must drop its ACK writer and socket");
    assert!(*progress.borrow() <= 3);
}

#[tokio::test]
async fn foreign_or_zero_ack_is_rejected_before_writing_or_advancing_progress() {
    let (opened, mut received, mut server) = opened_pair().await;
    retained(&mut server, 3, chunk()).await;
    let (receipt, _) = next_update(&mut received).await;
    let state = opened.bridge.output_ack_state().await;
    let progress = opened.bridge.output_progress().await.unwrap();
    let mut foreign = receipt.source.clone();
    foreign.stream_id = uuid::Uuid::from_u128(18).to_string();
    let mut zero = receipt.source.clone();
    zero.cursor = 0;
    for source in [foreign, zero] {
        assert!(matches!(
            opened
                .bridge
                .acknowledge_output(source, state.as_ref())
                .await,
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(*progress.borrow(), 2);
    }
    receipt.acknowledge().await.unwrap();
    ack(&mut server, 3).await;
    stop(opened).await;
}

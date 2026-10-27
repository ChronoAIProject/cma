use super::*;

#[tokio::test]
async fn receipt_loss_replays_the_same_terminal_until_acknowledged() {
    let output = OutputRetention::default();
    let terminal = br#"{"jsonrpc":"2.0","id":"prompt-1","result":{"stopReason":"end_turn"}}"#;
    let first_attachment = output.subscribe();
    output.retain(terminal).await;
    let first = output.snapshot(0).await.unwrap().frames;
    drop(first_attachment);
    output.finish().await;
    let _replacement_attachment = output.subscribe();
    let replay = output.snapshot(0).await.unwrap();
    assert_eq!(replay.frames, first);
    assert_eq!(replay.frames[0].cursor, 1);
    assert_eq!(replay.frames[0].bytes.as_ref(), terminal);
    assert!(!replay.finished_and_acknowledged);
    assert!(output.acknowledge(output.stream_id(), 1).await);
    let acknowledged = output.snapshot(0).await.unwrap();
    assert!(acknowledged.frames.is_empty());
    assert!(acknowledged.finished_and_acknowledged);
}

#[tokio::test]
async fn cumulative_acknowledgement_releases_only_its_prefix_and_accepts_duplicates() {
    let output = OutputRetention::default();
    for frame in [b"one".as_slice(), b"two", b"three"] {
        output.retain(frame).await;
    }
    assert!(output.acknowledge(output.stream_id(), 2).await);
    assert!(output.acknowledge(output.stream_id(), 1).await);
    let frames = output.snapshot(0).await.unwrap().frames;
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].cursor, 3);
    assert_eq!(frames[0].bytes.as_ref(), b"three");
    assert!(output.acknowledge(output.stream_id(), 3).await);
    assert_eq!(output.journal.lock().await.bytes, 0);
    output.retain(b"four").await;
    assert_eq!(output.snapshot(0).await.unwrap().frames[0].cursor, 4);
}

#[tokio::test]
async fn wrong_stream_zero_and_future_acknowledgements_preserve_output() {
    let output = OutputRetention::default();
    output.retain(b"held").await;
    assert!(!output.acknowledge(Uuid::new_v4(), 1).await);
    assert!(!output.acknowledge(output.stream_id(), 0).await);
    assert!(!output.acknowledge(output.stream_id(), 2).await);
    assert_eq!(output.snapshot(0).await.unwrap().frames.len(), 1);
}

#[tokio::test]
async fn delayed_consumer_recovers_more_than_the_legacy_broadcast_capacity() {
    let output = OutputRetention::default();
    let mut changed = output.subscribe();
    for cursor in 1_u64..=1_024 {
        output.retain(&cursor.to_be_bytes()).await;
    }
    changed.changed().await.unwrap();
    let frames = output.snapshot(0).await.unwrap().frames;
    assert_eq!(frames.len(), 1_024);
    assert_eq!(frames.first().unwrap().cursor, 1);
    assert_eq!(frames.last().unwrap().cursor, 1_024);
    assert!(output.snapshot(1_024).await.unwrap().frames.is_empty());
    // A reconnect has no last-sent cursor and receives the same entire prefix.
    assert_eq!(output.snapshot(0).await.unwrap().frames, frames);
}

#[tokio::test]
async fn full_byte_window_waits_without_losing_or_evicting_the_prefix() {
    let output = OutputRetention::default();
    let bytes = vec![0; BUFFER_BYTES - std::mem::size_of::<RetainedFrame>()];
    output.retain(&bytes).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), output.retain(b"next"))
            .await
            .is_err()
    );
    let held = output.snapshot(0).await.unwrap().frames;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].bytes.as_ref(), bytes);
    assert_eq!(output.journal.lock().await.bytes, BUFFER_BYTES);
    // Cancellation of a blocked retain does not reserve a cursor or lose data.
    assert!(output.acknowledge(output.stream_id(), 1).await);
    output.retain(b"next").await;
    let held = output.snapshot(0).await.unwrap().frames;
    assert_eq!(held[0].cursor, 2);
    assert_eq!(held[0].bytes.as_ref(), b"next");
}

#[tokio::test]
async fn full_frame_window_waits_and_receiver_disconnect_preserves_replay() {
    let output = OutputRetention::default();
    for _ in 0..MAX_RETAINED_FRAMES {
        output.retain(b"").await;
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), output.retain(b"last"))
            .await
            .is_err()
    );
    let attachment = output.subscribe();
    drop(attachment);
    assert_eq!(
        output.snapshot(0).await.unwrap().frames.len(),
        MAX_RETAINED_FRAMES
    );
    assert!(output.journal.lock().await.bytes < BUFFER_BYTES);
    assert!(
        output
            .acknowledge(output.stream_id(), MAX_RETAINED_FRAMES as u64)
            .await
    );
    output.retain(b"last").await;
    assert_eq!(
        output.snapshot(0).await.unwrap().frames[0].cursor,
        MAX_RETAINED_FRAMES as u64 + 1
    );
}

#[tokio::test]
async fn slow_durable_consumer_drains_more_than_both_retention_limits_in_order() {
    let output = Arc::new(OutputRetention::default());
    let producer = output.clone();
    let count = MAX_RETAINED_FRAMES as u64 + 37;
    let sending = tokio::spawn(async move {
        for cursor in 1..=count {
            let mut frame = vec![0; 1024];
            frame[..8].copy_from_slice(&cursor.to_be_bytes());
            producer.retain(&frame).await;
        }
        producer.finish().await;
    });
    // Make the real producer fill the 8 MiB window before consuming anything.
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert!(!sending.is_finished());
    let mut cursor = 0;
    let mut changed = output.subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while cursor < count {
            let snapshot = output.snapshot(cursor).await.unwrap();
            if snapshot.frames.is_empty() {
                changed.changed().await.unwrap();
                continue;
            }
            for frame in snapshot.frames {
                cursor += 1;
                assert_eq!(frame.cursor, cursor);
                assert_eq!(&frame.bytes[..8], &cursor.to_be_bytes());
                // Durable consumer receipt, cumulative only through this prefix.
                assert!(output.acknowledge(output.stream_id(), cursor).await);
            }
        }
    })
    .await
    .unwrap();
    sending.await.unwrap();
    assert!(
        output
            .snapshot(cursor)
            .await
            .unwrap()
            .finished_and_acknowledged
    );
}

#[tokio::test]
async fn choosing_legacy_output_releases_a_full_unselected_window() {
    let output = OutputRetention::default();
    output
        .retain(&vec![
            0;
            BUFFER_BYTES - std::mem::size_of::<RetainedFrame>()
        ])
        .await;
    output.select_legacy();
    tokio::time::timeout(
        std::time::Duration::from_millis(20),
        output.retain(b"legacy frame"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_oversized_or_unreadable_native_frame_permanently_invalidates_replay() {
    let output = OutputRetention::default();
    output.retain(b"valid prefix").await;
    output.gap().await;
    output.retain(b"later frame").await;
    assert_eq!(output.snapshot(0).await.err(), Some(2));
}

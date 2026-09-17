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
async fn byte_overflow_is_permanent_and_does_not_evict_unacknowledged_prefix() {
    let output = OutputRetention::default();
    let bytes = vec![0; BUFFER_BYTES - std::mem::size_of::<RetainedFrame>()];
    output.retain(&bytes).await;
    output.retain(b"overflow").await;
    assert_eq!(output.snapshot(0).await.err(), Some(1));
    {
        let journal = output.journal.lock().await;
        assert_eq!(journal.bytes, BUFFER_BYTES);
        assert_eq!(journal.held.len(), 1);
    }
    assert!(output.acknowledge(output.stream_id(), 1).await);
    assert!(!output.acknowledge(output.stream_id(), 2).await);
    output
        .retain(b"still unavailable after freeing space")
        .await;
    assert_eq!(output.snapshot(0).await.err(), Some(2));
}

#[tokio::test]
async fn empty_frames_cannot_exceed_the_retained_frame_count_bound() {
    let output = OutputRetention::default();
    for _ in 0..=MAX_RETAINED_FRAMES {
        output.retain(b"").await;
    }
    assert_eq!(output.snapshot(0).await.err(), Some(1));
    let journal = output.journal.lock().await;
    assert_eq!(journal.held.len(), MAX_RETAINED_FRAMES);
    assert!(journal.bytes < BUFFER_BYTES);
}

#[tokio::test]
async fn an_oversized_or_unreadable_native_frame_permanently_invalidates_replay() {
    let output = OutputRetention::default();
    output.retain(b"valid prefix").await;
    output.gap().await;
    output.retain(b"later frame").await;
    assert_eq!(output.snapshot(0).await.err(), Some(2));
}

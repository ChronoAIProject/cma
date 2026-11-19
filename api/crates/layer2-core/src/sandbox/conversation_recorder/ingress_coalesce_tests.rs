use super::*;

fn source(cursor: u64) -> RuntimeOutputSource {
    RuntimeOutputSource {
        stream_id: "11111111-1111-4111-8111-111111111111".into(),
        cursor,
        native_conversation_id: "n".repeat(4096),
        payload_sha256: "a".repeat(64),
    }
}

fn policy(max_frames: usize) -> AgentOutputCoalescePolicy {
    AgentOutputCoalescePolicy::new(Duration::from_millis(100), max_frames).unwrap()
}

fn retained(sources: Vec<RuntimeOutputSource>, deadline: Instant) -> AgentMessageAssembly {
    AgentMessageAssembly {
        output_state: Some(Box::new(OutputState {
            buffer: Some(QuietOutputBuffer {
                sources,
                generation: 7,
                deadline,
            }),
            committed_at: Some(deadline - Duration::from_millis(100)),
        })),
        ..Default::default()
    }
}

#[tokio::test(start_paused = true)]
async fn accepted_quiet_batch_owns_only_its_required_source_slots() {
    let deadline = Instant::now() + Duration::from_millis(100);
    for previous_count in [0, 1, 63, 126] {
        let previous = retained((1..=previous_count).map(source).collect(), deadline);
        let original = previous.clone();
        let incoming = source(previous_count + 1);
        let mut next = AgentMessageAssembly::default();
        assert!(next.retain_quiet(&previous, &incoming, 7, policy(128)));

        let state = next.output_state.as_ref().unwrap();
        let buffer = state.buffer.as_ref().unwrap();
        let expected: Vec<_> = (1..=previous_count + 1).map(source).collect();
        assert_eq!(buffer.sources, expected);
        assert_eq!(buffer.sources.capacity(), expected.len());
        assert_eq!(buffer.generation, 7);
        assert_eq!(buffer.deadline, deadline);
        assert_eq!(
            state.committed_at,
            original.output_state.as_ref().unwrap().committed_at
        );
        assert_eq!(previous, original);
        assert_ne!(
            buffer
                .sources
                .last()
                .unwrap()
                .native_conversation_id
                .as_ptr(),
            incoming.native_conversation_id.as_ptr(),
            "copied identities need their own storage allowance"
        );
        drop(previous);
        drop(incoming);
        assert_eq!(next.buffered_output(), expected.last());
    }
}

#[tokio::test(start_paused = true)]
async fn first_quiet_receipt_uses_one_slot_and_starts_its_original_deadline() {
    let now = Instant::now();
    let mut next = AgentMessageAssembly::default();
    assert!(next.retain_quiet(&AgentMessageAssembly::default(), &source(1), 7, policy(128)));
    let buffer = next.output_state.as_ref().unwrap().buffer.as_ref().unwrap();
    assert_eq!(buffer.sources.len(), 1);
    assert_eq!(buffer.sources.capacity(), 1);
    assert_eq!(buffer.deadline, now + Duration::from_millis(100));
}

#[tokio::test(start_paused = true)]
async fn full_or_expired_quiet_batch_preserves_both_assemblies_for_flush() {
    let now = Instant::now();
    for (count, max_frames, deadline) in [
        (0, 1, now + Duration::from_millis(100)),
        (1, 2, now + Duration::from_millis(100)),
        (127, 128, now + Duration::from_millis(100)),
        (1, 128, now),
        (1, 128, now - Duration::from_millis(1)),
    ] {
        let previous = retained((1..=count).map(source).collect(), deadline);
        let original = previous.clone();
        let mut next = AgentMessageAssembly {
            message_id: Some("next-message".into()),
            content: "uncommitted output".into(),
            ..Default::default()
        };
        let untouched = next.clone();
        assert!(!next.retain_quiet(&previous, &source(count + 1), 7, policy(max_frames)));
        assert_eq!(next, untouched);
        assert_eq!(previous, original);
    }
}

#[tokio::test(start_paused = true)]
async fn idle_gap_cannot_restart_the_age_of_the_last_durable_receipt() {
    let committed_at = Instant::now();
    let previous = AgentMessageAssembly {
        output_state: Some(Box::new(OutputState {
            buffer: None,
            committed_at: Some(committed_at),
        })),
        ..Default::default()
    };
    tokio::time::advance(Duration::from_millis(99)).await;
    let mut accepted = AgentMessageAssembly::default();
    assert!(accepted.retain_quiet(&previous, &source(1), 7, policy(128)));
    assert_eq!(
        accepted.output_flush_deadline(),
        Some(committed_at + Duration::from_millis(100))
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    let mut refused = AgentMessageAssembly::default();
    assert!(!refused.retain_quiet(&previous, &source(1), 7, policy(128)));
    assert_eq!(refused, AgentMessageAssembly::default());
}

use super::*;

#[tokio::test]
async fn small_result_above_both_history_limits_reads_only_original_references() {
    for noise in [10, 10_200] {
        let mut kinds = beginning();
        kinds.extend((0..noise).map(|_| AgentLogKind::ModeChanged {
            mode: "x".repeat(512),
        }));
        kinds.extend([
            message("small answer"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let f = make(kinds).await;
        if noise > 10_128 {
            assert!(f.agent.conversation.last_seq > 10_128);
            assert!(serde_json::to_vec(&f.source.data.rows).unwrap().len() > 4 * 1024 * 1024);
        }
        let expected = f.golden();
        let reading = f.reading.clone();
        let result = f
            .observe(move |source| {
                Box::pin(async move {
                    projection::indexed::complete(
                        source,
                        &crate::agent_page::AgentPageOrigin::test(),
                        reading,
                    )
                    .await
                })
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(f.source.counts.probes.load(Ordering::SeqCst), 1);
        assert_eq!(f.source.counts.points.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn deep_cursor_in_a_long_turn_seeks_without_reading_earlier_payloads() {
    let mut kinds = beginning();
    kinds.extend((0..12_000).map(|_| message("a")));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let reading = f.reading.clone();
    let next = 2 + 11_000 * 6 + 3;
    let page = f
        .observe(move |source| {
            Box::pin(async move {
                stream_page::read(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                    next,
                )
                .await
            })
        })
        .await
        .unwrap();
    assert_eq!(page.events[0]["sequence_number"], next);
    assert!(page.more && !page.pagination_required);
    assert!(f.source.counts.probes.load(Ordering::SeqCst) <= 16);
    assert!(f.source.counts.points.load(Ordering::SeqCst) <= 10);
    assert!(page.events.len() <= 8 * 6);
}

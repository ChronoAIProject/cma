use super::*;
use layer2_core::sandbox::agent_observation::observation_scope;

#[tokio::test]
async fn shared_indexed_evidence_preserves_models_tools_usage_and_every_replay_id() {
    let mut kinds = beginning();
    kinds.extend([
        message("first"),
        tool("call", "running"),
        output("call", "before terminal"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        update("call", "completed"),
        output("call", "after terminal"),
        AgentLogKind::Usage {
            input_tokens: 7,
            output_tokens: 11,
            context_used: 18,
            context_window: None,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 12,
            origin_key: None,
            model: Some("later-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "later request".into(),
        },
        AgentLogKind::Usage {
            input_tokens: 70,
            output_tokens: 110,
            context_used: 180,
            context_window: None,
        },
    ]);
    let f = make(kinds).await;
    let (reads, custody, _) =
        super::super::shared_tests::install(f.reads.clone(), f.store.clone(), &f.agent);
    let mut auth = reads
        .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
        .await
        .unwrap();
    let _watch = observation_scope(custody, async {
        reads.observe_output_stream(&mut auth).unwrap().unwrap()
    })
    .await;
    let expected = f.golden();
    let replay = projection::events(&expected, true);
    let mut warm = None;
    for _ in 0..2 {
        let reading = f.reading.clone();
        let actual = reads
            .with_output_observation(&auth, Some(3), move |source| {
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
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        for after in 0..=replay.len() {
            let reading = f.reading.clone();
            let values = reads
                .with_output_observation(&auth, Some(3), move |source| {
                    Box::pin(async move {
                        let mut next = after as u64;
                        let mut events = Vec::new();
                        loop {
                            let page = stream_page::read(
                                source,
                                &crate::agent_page::AgentPageOrigin::test(),
                                reading.clone(),
                                next,
                            )
                            .await?;
                            assert!(!page.pagination_required);
                            if let Some(last) = page.events.last() {
                                next = last["sequence_number"].as_u64().unwrap() + 1;
                            }
                            events.extend(page.events);
                            if page.terminal || !page.more {
                                break;
                            }
                        }
                        Ok(events)
                    })
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(values, replay[after..], "cursor {after}");
        }
        let counts = (
            f.source.counts.opens.load(Ordering::SeqCst),
            f.source.counts.points.load(Ordering::SeqCst),
            f.source.counts.probes.load(Ordering::SeqCst),
        );
        if let Some(warm) = warm {
            assert_eq!(
                counts, warm,
                "second subscriber must reuse verified evidence"
            );
        }
        warm = Some(counts);
    }
}

#[tokio::test]
async fn shared_output_generation_change_and_eviction_fail_closed_on_missing_evidence() {
    for evict in [false, true] {
        let mut kinds = beginning();
        kinds.extend([
            message("original"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let f = make(kinds).await;
        let (reads, custody, facts) =
            super::super::shared_tests::install(f.reads.clone(), f.store.clone(), &f.agent);
        let mut auth = reads
            .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
            .await
            .unwrap();
        let _watch = observation_scope(custody, async {
            reads.observe_output_stream(&mut auth).unwrap().unwrap()
        })
        .await;
        let reading = f.reading.clone();
        reads
            .with_output_observation(&auth, Some(3), move |source| {
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
            .unwrap()
            .unwrap();
        if evict {
            for n in 0..300u32 {
                let mut key = [0; 32];
                key[..4].copy_from_slice(&n.to_be_bytes());
                reads.memoize_verified_fold(key, || vec![n]);
            }
        } else {
            facts.generation.fetch_add(1, Ordering::SeqCst);
        }
        f.source.counts.fail.store(true, Ordering::SeqCst);
        let reading = f.reading.clone();
        assert!(matches!(
            reads
                .with_output_observation(&auth, Some(3), move |source| Box::pin(async move {
                    projection::indexed::complete(
                        source,
                        &crate::agent_page::AgentPageOrigin::test(),
                        reading,
                    )
                    .await
                }))
                .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn shared_large_output_keeps_every_bounded_page_and_refuses_complete_truncation() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"x".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let (reads, custody, _) =
        super::super::shared_tests::install(f.reads.clone(), f.store.clone(), &f.agent);
    let mut auth = reads
        .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
        .await
        .unwrap();
    let _watch = observation_scope(custody, async {
        reads.observe_output_stream(&mut auth).unwrap().unwrap()
    })
    .await;
    let reading = f.reading.clone();
    assert!(reads
        .with_output_observation(&auth, Some(3), move |source| Box::pin(async move {
            projection::indexed::complete(
                source,
                &crate::agent_page::AgentPageOrigin::test(),
                reading,
            )
            .await
        }))
        .await
        .unwrap()
        .unwrap()
        .is_none());
    let mut from = 0;
    let mut count = 0;
    loop {
        let page = reads
            .with_output_observation(&auth, Some(3), move |source| {
                Box::pin(async move { projection::indexed::output_page(source, from).await })
            })
            .await
            .unwrap()
            .unwrap();
        assert!(serde_json::to_vec(&page.items).unwrap().len() <= OUTPUT_PAGE_BYTES);
        count += page.items.len();
        if !page.more {
            break;
        }
        from = page.next;
    }
    assert_eq!(count, 12);
}

struct AdvanceOnceOnLazyOpen {
    source: Arc<Source>,
    opens: AtomicUsize,
}
#[async_trait]
impl AgentResponseLogStore for AdvanceOnceOnLazyOpen {
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        let ordinal = self.opens.fetch_add(1, Ordering::SeqCst);
        // The first capture primes metadata. Model the narrow, store-proven
        // advance on the later payload capture, after the memo's generation check.
        if ordinal == 1 {
            return Err(SandboxStoreError::CaptureAdvanced);
        }
        self.source.agent_output_reader(agent, turn).await
    }
    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        self.source.agent_response_log_window(agent, turn).await
    }
}

#[tokio::test]
async fn cached_indexed_metadata_does_not_erase_a_lazy_capture_advance() {
    use crate::responses::v2::read_cycle::{read_cycle, ReadFailure};
    use layer2_core::sandbox::{AgentEventStreamAuthorization, AgentResponseObservation};

    let mut kinds = beginning();
    kinds.push(message("answer"));
    let f = make(kinds).await;
    let source = Arc::new(AdvanceOnceOnLazyOpen {
        source: f.source.clone(),
        opens: AtomicUsize::new(0),
    });
    let (reads, custody, _) = super::super::shared_tests::install(
        f.reads.with_response_logs(Some(source.clone())),
        f.store.clone(),
        &f.agent,
    );
    let mut auth = reads
        .authorize_event_stream(&context("owner-a"), None, &f.agent.id)
        .await
        .unwrap();
    let _watch = observation_scope(custody, async {
        reads.observe_output_stream(&mut auth).unwrap().unwrap()
    })
    .await;
    // Opening pages need only certified metadata, so their committed memo has
    // neither changes nor payload points. Prime that actual MemoReader state.
    assert!(reads
        .read_response_observation(&auth, 3, None, |source| {
            Box::pin(async move { Ok(source.metadata().through) })
        })
        .await
        .is_ok());
    assert_eq!(source.opens.load(Ordering::SeqCst), 1);
    assert_eq!(f.source.counts.points.load(Ordering::SeqCst), 0);
    assert_eq!(f.source.counts.probes.load(Ordering::SeqCst), 0);

    struct Cycle<'a> {
        reads: &'a AgentReadService,
        auth: &'a AgentEventStreamAuthorization,
        reading: AgentResponseReading,
        attempts: usize,
    }
    let mut cycle = Cycle {
        reads: &reads,
        auth: &auth,
        reading: f.reading.clone(),
        attempts: 0,
    };
    let next = 2_u64;
    let result = read_cycle(
        &mut cycle,
        auth.read_deadline(),
        auth.revalidation_deadline(),
        |cycle| {
            Box::pin(async move {
                cycle.attempts += 1;
                let reading = cycle.reading.clone();
                let result = cycle
                    .reads
                    .read_response_observation(cycle.auth, 3, Some([1; 32]), move |source| {
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
                    .map_err(ReadFailure::from)?;
                match result {
                    AgentResponseObservation::Indexed(page) => Ok(page),
                    AgentResponseObservation::Legacy(_) => {
                        panic!("indexed observation must not fall back")
                    }
                }
            })
        },
        |_, failure, available| available && failure.advanced(),
    )
    .await;
    assert!(
        result.is_ok(),
        "store-proven lazy capture advance was folded into {:?}; attempts={}",
        result.as_ref().err().map(ReadFailure::diagnostic),
        cycle.attempts
    );
    let page = result.unwrap_or_else(|_| unreachable!());
    assert_eq!(cycle.attempts, 2);
    assert_eq!(page.events.first().unwrap()["sequence_number"], next);
    assert_eq!(source.opens.load(Ordering::SeqCst), 3);
    let reading = f.reading.clone();
    let warm = reads
        .read_response_observation(&auth, 3, Some([1; 32]), move |source| {
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
        .await;
    assert!(warm.is_ok());
    assert_eq!(
        source.opens.load(Ordering::SeqCst),
        3,
        "covered page reopened the store"
    );
}

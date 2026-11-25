use super::*;
use layer2_core::sandbox::{AgentEventStreamAuthorization, AgentResponseObservation};

#[path = "combined_observation_fixture.rs"]
mod counted;
use counted::CountedAccess;

async fn authorize(
    f: &IndexedFixture,
    shared: bool,
) -> (
    Arc<AgentReadService>,
    Arc<CountedAccess>,
    AgentEventStreamAuthorization,
) {
    if shared {
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: f.agent.id.clone(),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "combined-share".into(),
                },
            )
            .await
            .unwrap();
    }
    let access = Arc::new(CountedAccess {
        inner: f.store.clone(),
        calls: AtomicUsize::new(0),
    });
    let reads = f
        .reads
        .with_resource_access(Arc::new(ResourceAccessService::new(
            access.clone(),
            Arc::new(Authority),
        )));
    let held = reads
        .authorize_event_stream(
            &context(if shared { "reader" } else { "owner-a" }),
            shared.then_some("fixture-credential"),
            &f.agent.id,
        )
        .await
        .unwrap();
    access.calls.store(0, Ordering::SeqCst);
    (reads, access, held)
}

#[tokio::test]
async fn combined_indexed_projection_preserves_bytes_and_uses_two_authority_snapshots() {
    for shared in [false, true] {
        let mut kinds = beginning();
        kinds.extend([
            tool("call", "running"),
            output("call", "first"),
            message("interleaved"),
            update("call", "completed"),
            output("call", "latest"),
            message("final"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let f = make(kinds).await;
        let (reads, access, held) = authorize(&f, shared).await;
        let reading = f.reading.clone();
        let baseline = reads
            .with_output_observation(&held, Some(3), move |source| {
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
        access.calls.store(0, Ordering::SeqCst);
        f.source.counts.opens.store(0, Ordering::SeqCst);
        let reading = f.reading.clone();
        let result = reads
            .with_response_observation(&held, 3, move |source| {
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
            .unwrap();
        let AgentResponseObservation::Indexed(Some(result)) = result else {
            panic!("indexed result")
        };
        assert_eq!(
            serde_json::to_vec(&result).unwrap(),
            serde_json::to_vec(&baseline).unwrap()
        );
        assert_eq!(
            projection::events(&result, true),
            projection::events(&baseline, true)
        );
        assert_eq!(access.calls.load(Ordering::SeqCst), 2);
        assert_eq!(f.source.counts.opens.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn combined_indexed_pagination_and_projection_errors_never_fall_back() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"x".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let (reads, access, held) = authorize(&f, false).await;
    let reading = f.reading.clone();
    let result = reads
        .with_response_observation(&held, 3, move |source| {
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
        .unwrap();
    assert!(matches!(result, AgentResponseObservation::Indexed(None)));
    assert_eq!(access.calls.load(Ordering::SeqCst), 2);
    access.calls.store(0, Ordering::SeqCst);
    let result = reads
        .with_response_observation::<(), _>(&held, 3, |_| {
            Box::pin(async {
                Err(AgentReadError::Unavailable(
                    "source format changed; reconnect at the same durable cursor".into(),
                ))
            })
        })
        .await;
    assert!(
        matches!(result, Err(AgentReadError::Unavailable(message)) if message.starts_with("source format changed"))
    );
    assert_eq!(access.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn combined_indexed_metadata_failure_never_invokes_projection_or_fallback() {
    let mut f = make({
        let mut kinds = beginning();
        kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
        kinds
    })
    .await;
    f.mutate(|data| data.metadata.through += 1);
    let (reads, access, held) = authorize(&f, false).await;
    let result = reads
        .with_response_observation::<(), _>(&held, 3, |_| {
            panic!("corrupt metadata reached projection")
        })
        .await;
    assert!(matches!(result, Err(AgentReadError::Corrupt(_))));
    assert_eq!(access.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn combined_indexed_release_overrides_success_and_projection_failure_after_deletion() {
    for failure in [false, true] {
        let mut kinds = beginning();
        kinds.extend([
            message("private answer"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let f = make(kinds).await;
        let (reads, access, held) = authorize(&f, false).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let source_entered = entered.clone();
        let source_resume = resume.clone();
        let reading = f.reading.clone();
        let (result, ()) = tokio::join!(
            reads.with_response_observation(&held, 3, move |source| Box::pin(async move {
                let result = projection::indexed::complete(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                )
                .await;
                source_entered.notify_one();
                source_resume.notified().await;
                if failure {
                    Err(AgentReadError::Unavailable(
                        "private projection failure".into(),
                    ))
                } else {
                    result
                }
            })),
            async {
                entered.notified().await;
                f.purge().await;
                resume.notify_one();
            }
        );
        assert!(matches!(result, Err(AgentReadError::NotFound)));
        assert_eq!(access.calls.load(Ordering::SeqCst), 2);
    }
}

#[path = "combined_observation_http.rs"]
mod http;

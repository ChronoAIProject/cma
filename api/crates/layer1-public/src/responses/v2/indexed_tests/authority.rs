use super::*;

#[tokio::test]
async fn deletion_while_points_wait_discards_the_private_result() {
    let mut kinds = beginning();
    kinds.extend([
        message("private result"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    f.source.counts.stall.store(true, Ordering::SeqCst);
    let reads = f.reads.clone();
    let id = f.agent.id.clone();
    let reading = f.reading.clone();
    let pending = tokio::spawn(async move {
        reads
            .with_owned_output_observation(&context("owner-a").caller, &id, 3, move |source| {
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
    });
    f.source.counts.entered.notified().await;
    f.purge().await;
    f.source.counts.stall.store(false, Ordering::SeqCst);
    f.source.counts.release.notify_one();
    assert!(pending.await.unwrap().is_err());
}

#[tokio::test]
async fn adopted_unbound_stream_keeps_reading_with_legacy_optimization_off() {
    let mut kinds = beginning();
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let data = make(kinds).await;
    let f = fixture_with_events(Some(data.source.clone()), false).await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        data.source.counts.opens.load(Ordering::SeqCst),
        0,
        "opening headers must not load captured source"
    );
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    let command = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &command.receipt.envelope.command_id,
            command.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.failed"
    );
    assert!(body.next().await.is_none());
}

async fn shared(
    f: &IndexedFixture,
) -> (
    ResourceShare,
    layer2_core::sandbox::AgentEventStreamAuthorization,
) {
    let share = f
        .access
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
                idempotency_key: "indexed-share".into(),
            },
        )
        .await
        .unwrap();
    let held = f
        .reads
        .authorize_event_stream(&context("reader"), Some("fixture-credential"), &f.agent.id)
        .await
        .unwrap();
    (share.share, held)
}

#[tokio::test]
async fn share_revocation_during_source_read_discards_the_result() {
    let mut kinds = beginning();
    kinds.extend([
        message("private"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let (share, held) = shared(&f).await;
    f.source.counts.stall.store(true, Ordering::SeqCst);
    let reads = f.reads.clone();
    let reading = f.reading.clone();
    let task = tokio::spawn(async move {
        reads
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
    });
    f.source.counts.entered.notified().await;
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "indexed-revoke".into(),
            },
        )
        .await
        .unwrap();
    f.source.counts.stall.store(false, Ordering::SeqCst);
    f.source.counts.release.notify_one();
    assert!(task.await.unwrap().is_err());
}

#[tokio::test(start_paused = true)]
async fn a_slow_indexed_read_cannot_extend_the_original_deadline() {
    let mut kinds = beginning();
    kinds.extend([
        message("private"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let (_, held) = shared(&f).await;
    f.source.counts.stall.store(true, Ordering::SeqCst);
    let reads = f.reads.clone();
    let reading = f.reading.clone();
    let task = tokio::spawn(async move {
        reads
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
    });
    f.source.counts.entered.notified().await;
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(task.await.unwrap().is_err());
}

#[tokio::test]
async fn retired_agent_history_remains_available_until_purge() {
    let mut kinds = beginning();
    kinds.extend([
        message("retained"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let mut retired = f.agent.clone();
    retired.status = AgentStatus::Deleted;
    retired.deleted_at = Some(AT.into());
    retired.revision += 1;
    f.store.save_agent(f.agent.revision, retired).await.unwrap();
    let reading = f.reading.clone();
    let actual = f
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
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(f.golden()).unwrap()
    );
}

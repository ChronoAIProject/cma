use super::*;

#[tokio::test]
async fn ownership_change_while_points_wait_discards_the_private_result() {
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
    let mut changed = f.agent.clone();
    changed.owner = SubjectId::new("owner-b");
    changed.revision += 1;
    f.store.save_agent(f.agent.revision, changed).await.unwrap();
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

use super::*;

async fn another_original(setup: &Setup, index: usize) -> Receipt {
    let raw = native::text(
        &format!("om_original_{index}"),
        "private source text",
        now(),
    );
    let msg = message::extract(&setup.source, &raw, now())
        .unwrap()
        .unwrap();
    let request = message::launch(
        setup.native.channel.clone(),
        &setup.source,
        &msg,
        None,
        &routing_plan::fixture::keys(),
        Instant::now(),
        now(),
    )
    .unwrap();
    let routing_plan::OrdinaryPlan::Routed(plan) = routing_plan::plan(
        &request,
        &routing_plan::fixture::keys(),
        now(),
        routing_plan::RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &BTreeSet::new(),
        },
    )
    .unwrap() else {
        panic!()
    };
    let candidate = Candidate::prepare(
        &request,
        &plan,
        b"native read fixture",
        &routing_plan::fixture::keys(),
        now(),
    )
    .unwrap();
    setup.native.server.store().admit(&candidate).await.unwrap();
    setup
        .activity
        .received_at(
            &setup.receipt.original.owner,
            setup.receipt.original.record_id.as_str(),
            &request.activity,
            time::OffsetDateTime::from_unix_timestamp(setup.receipt.confirmed_at).unwrap(),
        )
        .await
        .unwrap();
    setup
        .activity
        .change(
            setup.receipt.original.record_id.as_str(),
            request.activity.event_id.as_str(),
            &Change::state(
                setup.receipt.consumer.as_str(),
                ConsumerState::Completed,
                None,
            ),
        )
        .await
        .unwrap();
    let mut receipt = setup.receipt.clone();
    receipt.original.event_id = request.activity.event_id;
    receipt
}

#[tokio::test]
async fn full_batch_across_original_plans_commits_once_with_record_latency_and_replays_without_writes(
) {
    let setup = Setup::new(false, false, LarkRegion::Feishu, false).await;
    let mut receipts = Vec::new();
    // Sixteen distinct retained plans, two independently confirmed messages
    // each, keep the full 32-ID callback inside existing activity row limits.
    for index in 0..16 {
        let original = if index == 0 {
            setup.receipt.clone()
        } else {
            another_original(&setup, index).await
        };
        for part in 0..2 {
            let mut receipt = original.clone();
            receipt.message_id = format!("om_batch_{index}_{part}");
            receipt.reply.message_id = receipt.message_id.clone();
            setup.store().put(&receipt).await.unwrap();
            receipts.push(receipt);
        }
    }
    let messages: Vec<_> = receipts
        .iter()
        .map(|receipt| receipt.message_id.as_str())
        .collect();
    let raw = setup.event(0, &messages);
    setup
        .native
        .server
        .memory
        .delay_ms
        .store(100, Ordering::SeqCst);
    let writes = setup.native.server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        setup.call(&setup.state, &raw, true).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        setup.native.server.memory.writes.load(Ordering::SeqCst),
        writes + 1
    );
    let page = setup.page().await;
    assert_eq!(page.items.len(), 16);
    for receipt in &receipts {
        let row = page
            .items
            .iter()
            .find(|row| row.event_id == receipt.original.event_id.as_str())
            .unwrap();
        let consumer = row
            .consumers
            .iter()
            .find(|consumer| consumer.consumer_id == receipt.consumer.as_str())
            .unwrap();
        assert_eq!(consumer.state, ConsumerState::Completed);
        assert_eq!(consumer.deliveries.len(), 2);
        assert!(consumer
            .deliveries
            .iter()
            .any(|delivery| delivery.platform_message_id.as_deref()
                == Some(receipt.message_id.as_str())
                && delivery.outcome == "read"));
    }
    let before = serde_json::to_value(&page).unwrap();
    // Fresh observer has no process-local activity context or receipt progress.
    let restarted = setup
        .state
        .clone()
        .with_activity_observer(Arc::new(ObservedActivity::new(
            Arc::new(ActivityStore::new(
                setup.native.server.records.clone(),
                Arc::new(routing_plan::fixture::keys()),
            )),
            Arc::new(NoopObserver),
        )));
    assert_eq!(
        setup.call(&restarted, &raw, true).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        setup.native.server.memory.writes.load(Ordering::SeqCst),
        writes + 1
    );
    assert_eq!(serde_json::to_value(setup.page().await).unwrap(), before);
}

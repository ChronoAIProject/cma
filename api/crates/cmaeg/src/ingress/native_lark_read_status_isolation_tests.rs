use super::*;

#[tokio::test]
async fn mixed_batch_retains_known_read_while_unknown_target_waits_for_its_own_receipt() {
    let setup = Setup::new(false, false, LarkRegion::Feishu, true).await;
    let mut foreign = setup.receipt.clone();
    foreign.message_id = "om_foreign".into();
    foreign.reply.message_id = foreign.message_id.clone();
    foreign.recipient = "oc_other".into();
    foreign.reply.chat_id = foreign.recipient.clone();
    setup.store().put(&foreign).await.unwrap();
    let raw = setup.event(0, &["om_foreign", "om_pending", "om_confirmed"]);
    assert_eq!(
        setup.call(&setup.state, &raw, true).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let page = setup.page().await;
    let consumer = page.items[0]
        .consumers
        .iter()
        .find(|c| c.consumer_id == setup.receipt.consumer.as_str())
        .unwrap();
    assert_eq!(consumer.deliveries.len(), 1);
    assert_eq!(
        consumer.deliveries[0].platform_message_id.as_deref(),
        Some("om_confirmed")
    );
    let first = serde_json::to_value(&consumer.deliveries[0]).unwrap();
    let mut receipt = setup.receipt.clone();
    receipt.message_id = "om_pending".into();
    receipt.reply.message_id = receipt.message_id.clone();
    setup.store().put(&receipt).await.unwrap();
    assert_eq!(
        setup.call(&setup.state, &raw, true).await.status(),
        StatusCode::OK
    );
    let page = setup.page().await;
    let consumer = page.items[0]
        .consumers
        .iter()
        .find(|c| c.consumer_id == setup.receipt.consumer.as_str())
        .unwrap();
    assert_eq!(consumer.deliveries.len(), 2);
    assert_eq!(
        serde_json::to_value(&consumer.deliveries[0]).unwrap(),
        first
    );
    assert_eq!(
        consumer.deliveries[1].platform_message_id.as_deref(),
        Some("om_pending")
    );
}

#[tokio::test]
async fn expired_and_corrupt_confirmed_records_never_gain_read_status() {
    for expired in [false, true] {
        let setup = Setup::new(false, false, LarkRegion::Feishu, true).await;
        let id = {
            let rows = setup.native.server.memory.rows.lock().unwrap();
            rows.iter()
                .find_map(|(id, row)| {
                    let bytes = cmaeg_core::seal::open_interaction(
                        &routing_plan::fixture::keys(),
                        id,
                        &row.blob,
                    )
                    .unwrap()
                    .payload;
                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                    (value["kind"] == "native_lark_receipt_v1").then(|| id.clone())
                })
                .unwrap()
        };
        setup.native.server.change(&id, |receipt| {
            if expired {
                receipt["confirmed_at"] = (now() - 60).into();
                receipt["retained_until"] = (now() - 1).into();
            } else {
                receipt["recipient"] = "oc_corrupt".into();
            }
        });
        assert_eq!(
            setup
                .call(&setup.state, &setup.event(0, &["om_confirmed"]), true)
                .await
                .status(),
            if expired {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        setup.assert_unread().await;
    }
}

#[tokio::test]
async fn owner_chat_thread_provider_and_revision_mismatches_never_annotate() {
    for mutation in [
        "owner",
        "record",
        "channel",
        "chat",
        "thread",
        "provider",
        "revision",
        "future_receipt",
    ] {
        let mut setup = Setup::new(false, false, LarkRegion::Feishu, false).await;
        match mutation {
            "owner" => setup.receipt.original.owner = "another_owner".into(),
            "record" => {
                setup.receipt.original.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap()
            }
            "channel" => {
                setup.receipt.original.channel_id = "cmaeg1.ch.another-fixture".parse().unwrap()
            }
            "chat" => {
                setup.receipt.recipient = "oc_other".into();
                setup.receipt.reply.chat_id = "oc_other".into();
            }
            "thread" => setup.receipt.reply.thread_id = Some("omt_other".into()),
            "provider" => setup.receipt.provider.binding_id = "bnd_other".parse().unwrap(),
            "revision" => setup.receipt.reply.revision = "0".repeat(64),
            "future_receipt" => setup.receipt.confirmed_at = now() + 600,
            _ => unreachable!(),
        }
        setup.store().put(&setup.receipt).await.unwrap();
        assert_eq!(
            setup
                .call(&setup.state, &setup.event(121, &["om_confirmed"]), true)
                .await
                .status(),
            StatusCode::OK,
            "{mutation}"
        );
        // The original channel activity is addressed using its actual owner/record.
        let page = setup
            .activity
            .latest(
                &setup.native.channel.definition.owner.subject,
                setup.native.channel.definition.record_id.as_str(),
            )
            .await
            .unwrap();
        assert!(
            page.items[0]
                .consumers
                .iter()
                .all(|c| c.deliveries.is_empty()),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn missing_storage_or_observer_retries_only_observation_and_revocation_stays_closed() {
    let mut setup = Setup::new(false, false, LarkRegion::Feishu, true).await;
    let raw = setup.event(0, &["om_confirmed"]);
    *setup
        .native
        .server
        .memory
        .unavailable_read_id
        .lock()
        .unwrap() = Some(setup.receipt.original.admission_id());
    assert_eq!(
        setup.call(&setup.state, &raw, true).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    *setup
        .native
        .server
        .memory
        .unavailable_read_id
        .lock()
        .unwrap() = None;
    let unavailable = setup
        .state
        .clone()
        .with_activity_observer(Arc::new(NoopObserver));
    assert_eq!(
        setup.call(&unavailable, &raw, true).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    setup.assert_unread().await;
    let disabled = setup.state.clone().with_native_lark_intake(false);
    assert_eq!(
        setup.call(&disabled, &raw, true).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    Arc::make_mut(&mut setup.native.channel)
        .definition
        .secrets
        .native_lark
        .as_mut()
        .unwrap()
        .app_secret = cmaeg_core::channel::Secret::new("rotated-app-secret-fixture-12345");
    setup.native.save_channel();
    assert_eq!(
        setup.call(&setup.state, &raw, true).await.status(),
        StatusCode::OK
    );
    setup.assert_unread().await;
}

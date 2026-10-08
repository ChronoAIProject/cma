//! Native provisioning, activation and retirement contracts.
use super::*;

#[tokio::test]
async fn managed_creation_reserves_before_button_then_verifies_exact_child_and_keeps_token_private()
{
    let app = Native::new().await;
    app.begin(false).await;
    assert!(app.peer.reserved_before_button.load(Ordering::SeqCst));
    let receipt = app.receipt(42, now(), 101);
    let cmaeg_core::activity::ActivityKind::Event { ce, .. } = &receipt.kind else {
        unreachable!()
    };
    assert_eq!(ce.data.as_ref().unwrap()["bot"]["id"], "88");
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    let (record, _) = app.stored().await;
    assert_eq!(record.status, Status::Ready);
    assert_eq!(record.material.as_ref().unwrap().bot_id, "88");
    assert_eq!(
        app.methods(),
        [
            "getMe",
            "sendMessage",
            "getMe",
            "getManagedBotToken",
            "getMe"
        ]
    );
    let view = serde_json::to_string(
        &app.service
            .get(&owner("alice", false), &record.id)
            .await
            .unwrap(),
    )
    .unwrap();
    for secret in [
        "88:child-fixture",
        "77:manager-fixture",
        &record.material.as_ref().unwrap().webhook_secret,
    ] {
        assert!(!view.contains(secret));
    }
    let before = app.methods();
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
}

#[tokio::test]
async fn other_actor_old_and_cancelled_receipts_cannot_complete_pending_creation() {
    let app = Native::new().await;
    app.begin(false).await;
    let before = app.methods();
    app.service
        .observe_creation(&app.channel, &app.receipt(43, now(), 102))
        .await
        .unwrap();
    app.service
        .observe_creation(
            &app.channel,
            &app.receipt(42, app.record.created_at - 1, 103),
        )
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
    assert_eq!(app.stored().await.0.status, Status::AwaitingPlatform);
    app.service
        .cancel(&owner("alice", false), &app.record.id)
        .await
        .unwrap();
    app.service
        .observe_creation(&app.channel, &app.receipt(42, now(), 104))
        .await
        .unwrap();
    assert_eq!(app.methods(), before);
    assert_eq!(app.stored().await.0.status, Status::Cancelled);
}

#[tokio::test]
async fn child_identity_mismatch_retains_receipt_and_retry_uses_the_same_operation() {
    let app = Native::new().await;
    app.begin(false).await;
    let receipt = app.receipt(42, now(), 105);
    app.peer.mismatched_child.store(true, Ordering::SeqCst);
    assert!(matches!(
        app.service.observe_creation(&app.channel, &receipt).await,
        Err(Error::Invalid(_))
    ));
    let (record, _) = app.stored().await;
    assert_eq!(record.status, Status::AwaitingPlatform);
    assert!(record.material.is_none());
    let (Stored::Pending(pending), _) = app
        .fixture
        .store()
        .load(&app.peer.pending_id)
        .await
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(pending.receipt.unwrap().event_id, receipt.event_id.as_str());
    app.peer.mismatched_child.store(false, Ordering::SeqCst);
    app.service
        .observe_creation(&app.channel, &receipt)
        .await
        .unwrap();
    assert_eq!(app.stored().await.0.status, Status::Ready);
}

#[tokio::test]
async fn child_requested_as_manager_waits_for_real_management_capability() {
    let app = Native::new().await;
    app.begin(true).await;
    app.service
        .observe_creation(&app.channel, &app.receipt(42, now(), 106))
        .await
        .unwrap();
    let (record, version) = app.stored().await;
    assert_eq!(record.status, Status::AwaitingPlatform);
    assert!(record.material.is_some());
    let (record, version) = app
        .service
        .refresh_management(record, version)
        .await
        .unwrap();
    assert_eq!(record.status, Status::AwaitingPlatform);
    app.peer.child_enabled.store(true, Ordering::SeqCst);
    let (record, _) = app
        .service
        .refresh_management(record, version)
        .await
        .unwrap();
    assert_eq!(record.status, Status::Ready);
    assert!(record.material.unwrap().can_manage_bots);
}

#[tokio::test]
async fn disabled_parent_management_refuses_before_pending_or_platform_button() {
    let app = Native::new().await;
    app.peer.manager_enabled.store(false, Ordering::SeqCst);
    assert!(matches!(
        app.service
            .create_managed(
                &app.record,
                1,
                app.record.origin.event.record_id.as_str(),
                None,
                None,
                false
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(app.methods(), ["getMe"]);
    assert!(matches!(
        app.fixture.store().load(&app.peer.pending_id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(app.stored().await.0.status, Status::Draft);
}

#[tokio::test]
async fn existing_webhook_is_preserved_and_uncertain_activation_retries_the_retained_pointer() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    *app.peer.webhook.lock().unwrap() = "https://external.example/keep-this-route".into();
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        "https://external.example/keep-this-route"
    );
    assert!(!app.methods().contains(&"setWebhook".into()));
    assert_eq!(
        app.stored().await.0.channel_id.as_deref(),
        Some(channel_id.as_str())
    );
    app.peer.webhook.lock().unwrap().clear();
    app.peer.lose_webhook_reply.store(true, Ordering::SeqCst);
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    let expected = format!("https://gateway.example/v1/telegram/{channel_id}");
    assert_eq!(*app.peer.webhook.lock().unwrap(), expected);
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    app.expire_lease().await;
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    assert_eq!(
        app.stored().await.0.channel_id.as_deref(),
        Some(channel_id.as_str())
    );
    let other: ChannelId = app.fixture.retain_channel(&definition);
    assert_ne!(other, channel_id);
    assert!(
        binding::activate(&app.service.management, &definition, &other)
            .await
            .is_err()
    );
    let urls: Vec<_> = app
        .peer
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "setWebhook")
        .map(|(_, body)| body["url"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(urls, [expected.clone(), expected]);
}

#[tokio::test]
async fn deletion_fences_delayed_activation_and_keeps_retirement_until_it_finishes() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task_definition = definition.clone();
    let task_id = channel_id.clone();
    let activating =
        tokio::spawn(async move { binding::activate(&state, &task_definition, &task_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    binding::disconnect(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    app.fixture.forget_channel(definition.record_id.as_str());
    binding::release(&app.service.management, &definition)
        .await
        .unwrap();
    let (retiring, _) = app.stored().await;
    assert!(retiring.webhook_retiring);
    assert!(retiring.webhook_lease.is_some());
    assert_eq!(retiring.channel_id.as_deref(), Some(channel_id.as_str()));
    assert!(binding::expand_create(
        &app.service.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition)
    )
    .await
    .is_err());
    app.peer.webhook_resume.notify_one();
    assert!(activating.await.unwrap().is_err());
    assert!(app.peer.webhook.lock().unwrap().is_empty());
    let (retired, _) = app.stored().await;
    assert!(!retired.webhook_retiring);
    assert!(retired.webhook_lease.is_none());
    assert!(retired.channel_id.is_none());
    assert!(retired.channel_record.is_none());
    binding::expand_create(
        &app.service.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn restart_reconciles_abandoned_activation_and_uncertain_delete_before_rebinding() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task_definition = definition.clone();
    let task_id = channel_id.clone();
    let activating =
        tokio::spawn(async move { binding::activate(&state, &task_definition, &task_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    activating.abort();
    assert!(activating.await.unwrap_err().is_cancelled());
    binding::disconnect(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    app.fixture.forget_channel(definition.record_id.as_str());
    binding::release(&app.service.management, &definition)
        .await
        .unwrap();
    app.peer.webhook_resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_finished.notified())
        .await
        .unwrap();
    assert_eq!(
        *app.peer.webhook.lock().unwrap(),
        format!("https://gateway.example/v1/telegram/{channel_id}")
    );
    let mut restarted = app.fixture.service();
    restarted.management.telegram = app.service.management.telegram.clone();
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    app.expire_lease().await;
    app.peer.lose_delete_reply.store(true, Ordering::SeqCst);
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Unavailable)
    ));
    let (uncertain, _) = app.stored().await;
    assert!(uncertain.webhook_retiring);
    assert_eq!(uncertain.channel_id.as_deref(), Some(channel_id.as_str()));
    assert!(matches!(
        restarted.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    app.expire_lease().await;
    restarted
        .reconcile_webhook_retirement(&app.record.id)
        .await
        .unwrap();
    let (retired, _) = app.stored().await;
    assert!(retired.channel_id.is_none());
    assert!(retired.webhook_lease.is_none());
    assert!(!retired.webhook_retiring);
    assert!(app.peer.webhook.lock().unwrap().is_empty());
    binding::expand_create(
        &restarted.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition),
    )
    .await
    .unwrap();
}

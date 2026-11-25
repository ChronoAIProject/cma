//! Expiry cannot cancel a request already accepted by Telegram.
use super::*;
#[path = "legacy_retirement_custody_tests.rs"]
mod legacy_retirement;

#[tokio::test]
async fn expired_unknown_activation_keeps_pointers_and_refuses_successor() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task_definition = definition.clone();
    let task_id = channel_id.clone();
    let task =
        tokio::spawn(async move { binding::activate(&state, &task_definition, &task_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    app.expire_lease().await;
    let original = app.stored().await.0.webhook_lease.unwrap().nonce;
    let mut other = app.fixture.service();
    other.management.telegram = app.service.management.telegram.clone();
    binding::disconnect(&other.management, &definition, &channel_id)
        .await
        .unwrap();
    app.fixture
        .forget_channel(definition.record_id.as_str())
        .await;
    binding::release(&other.management, &definition)
        .await
        .unwrap();
    let held = app.stored().await.0;
    assert_eq!(
        held.webhook_lease.as_ref().map(|lease| &lease.nonce),
        Some(&original)
    );
    assert_eq!(held.channel_id.as_deref(), Some(channel_id.as_str()));
    assert!(held.webhook_retiring);
    assert!(binding::expand_create(
        &other.management,
        &owner("alice", false),
        &mut app.replacement_request(&definition)
    )
    .await
    .is_err());
    app.peer.webhook_resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_finished.notified())
        .await
        .unwrap();
    assert!(matches!(
        other.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    assert!(!app.methods().contains(&"deleteWebhook".into()));
}

#[tokio::test]
async fn expired_unknown_delete_does_not_release_or_repeat_remote_mutation() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    binding::activate(&app.service.management, &definition, &channel_id)
        .await
        .unwrap();
    app.peer.lose_delete_reply.store(true, Ordering::SeqCst);
    assert!(
        binding::disconnect(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    app.expire_lease().await;
    let nonce = app.stored().await.0.webhook_lease.unwrap().nonce;
    app.fixture
        .forget_channel(definition.record_id.as_str())
        .await;
    let mut other = app.fixture.service();
    other.management.telegram = app.service.management.telegram.clone();
    assert!(matches!(
        other.reconcile_webhook_retirement(&app.record.id).await,
        Err(Error::Conflict)
    ));
    let held = app.stored().await.0;
    assert_eq!(
        held.webhook_lease.as_ref().map(|lease| &lease.nonce),
        Some(&nonce)
    );
    assert!(held.channel_record.is_some());
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "deleteWebhook")
            .count(),
        1
    );
}

#[tokio::test]
async fn exact_confirmed_completion_can_settle_after_expiry() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let task =
        tokio::spawn(async move { binding::activate(&state, &definition, &channel_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();
    app.expire_lease().await;
    app.peer.webhook_resume.notify_one();
    task.await.unwrap().unwrap();
    assert!(app.stored().await.0.webhook_lease.is_none());
}

#[tokio::test]
async fn terminal_rate_limit_refusals_release_exact_custody_for_retry() {
    for delete in [false, true] {
        let app = Native::new().await;
        let (definition, channel_id) = app.reserve_child().await;
        if delete {
            binding::activate(&app.service.management, &definition, &channel_id)
                .await
                .unwrap();
        }
        app.peer.rate_limit.store(true, Ordering::SeqCst);
        let attempt = if delete {
            binding::disconnect(&app.service.management, &definition, &channel_id).await
        } else {
            binding::activate(&app.service.management, &definition, &channel_id).await
        };
        assert!(
            matches!(attempt, Err(crate::channels::management::ManagementError::Provider(crate::providers::ProviderError::Busy { retry_after: Some(delay) })) if delay == Duration::from_secs(7))
        );
        assert!(app.stored().await.0.webhook_lease.is_none());
        if delete {
            binding::disconnect(&app.service.management, &definition, &channel_id)
                .await
                .unwrap();
        } else {
            binding::activate(&app.service.management, &definition, &channel_id)
                .await
                .unwrap();
        }
        assert!(app.stored().await.0.webhook_lease.is_none());
    }
}

#[tokio::test]
async fn local_presend_refusal_does_not_retain_remote_custody() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    let (mut record, version) = app.stored().await;
    record.material.as_mut().unwrap().webhook_secret = "invalid secret".into();
    app.fixture
        .store()
        .save_connection(&record, Some(version))
        .await
        .unwrap();
    assert!(
        binding::activate(&app.service.management, &definition, &channel_id)
            .await
            .is_err()
    );
    assert!(app.stored().await.0.webhook_lease.is_none());
    assert!(!app.methods().contains(&"setWebhook".into()));
}

#[tokio::test]
async fn legacy_disconnect_retries_exact_record_after_a_competing_webhook_lease() {
    let app = Native::new().await;
    let (definition, channel_id) = app.reserve_child().await;
    app.peer.pause_webhook.store(true, Ordering::SeqCst);
    let state = app.service.management.clone();
    let activating = definition.clone();
    let active =
        tokio::spawn(async move { binding::activate(&state, &activating, &channel_id).await });
    tokio::time::timeout(Duration::from_secs(2), app.peer.webhook_started.notified())
        .await
        .unwrap();

    let Err(conflict) = binding::disconnect_owned(&app.service.management, &definition, None).await
    else {
        panic!("a held webhook lease must defer a legacy disconnect");
    };
    assert!(!conflict.unconfirmed);
    assert!(!app.methods().contains(&"deleteWebhook".into()));

    app.peer.pause_webhook.store(false, Ordering::SeqCst);
    app.peer.webhook_resume.notify_one();
    let _ = tokio::time::timeout(Duration::from_secs(2), active)
        .await
        .unwrap()
        .unwrap()
        .ok();
    assert!(app.stored().await.0.webhook_lease.is_none());
    assert!(
        binding::disconnect_owned(&app.service.management, &definition, None)
            .await
            .is_ok()
    );
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| *method == "deleteWebhook")
            .count(),
        1
    );
}

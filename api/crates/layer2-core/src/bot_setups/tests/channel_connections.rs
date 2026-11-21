use super::*;

async fn connect(f: &mut Fixture) -> SetupView {
    let view = f.services("personal").await;
    let view = f.action(&view, "done").await;
    assert_eq!(view.step, SetupStep::ExternalBot);
    f.action(&view, "register").await
}

#[tokio::test]
async fn connection_registration_is_durable_and_notification_stops_before_consent() {
    let mut f = Fixture::new();
    let pending = connect(&mut f).await;
    let stored = f.store.get(&f.actor).await.unwrap().unwrap();
    let request = stored.pending_connection.unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(&request.record_id)
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(request.source_event_ref, "cmaeg1.ev.fixture");
    assert_eq!(pending.step, SetupStep::ExternalBot);
    assert!(pending
        .connection_url
        .as_ref()
        .unwrap()
        .ends_with(&request.record_id));
    // Restart reads the same persisted admission rather than creating a new intent.
    f.service = BotSetupService::new(f.store.clone(), f.dependencies.clone());
    let view = f.action(&pending, "refresh").await;
    assert_eq!(view.step, SetupStep::ExternalBot);
    let calls = f.dependencies.channel_calls.lock().unwrap().clone();
    assert_eq!(calls, vec![request.record_id.clone(); 2]);

    f.dependencies.channel_ready.store(true, Ordering::SeqCst);
    let notification = f.event(SetupInput::Notification);
    let ready = f.advance(&notification).await.unwrap();
    assert_eq!(ready.step, SetupStep::Admission);
    assert_eq!(
        ready.connection_ref.as_deref(),
        Some(request.record_id.as_str())
    );
    assert!(ready.confirmation_digest.is_none());
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
    let review = f.action(&ready, "private").await;
    let notification = f.event(SetupInput::Notification);
    let no_consent = f
        .service
        .act(
            "binding",
            "binding-fixture",
            &notification,
            SetupActionRequest {
                request_id: "notification-confirm".into(),
                expected: Some(SetupExpected {
                    setup_id: review.setup_id.clone(),
                    revision: review.revision,
                }),
                action: SetupAction::Confirm {
                    plan_digest: review.confirmation_digest.clone().unwrap(),
                },
            },
        )
        .await;
    assert!(matches!(no_consent, Err(BotError::Forbidden)));
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
    let consent = f.confirm(&review);
    assert_eq!(
        f.advance(&consent).await.unwrap().step,
        SetupStep::Provisioning
    );
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancel_or_edit_detaches_the_old_receipt_before_late_completion() {
    for action in ["cancel", "edit"] {
        let mut f = Fixture::new();
        let pending = connect(&mut f).await;
        let old = f
            .store
            .get(&f.actor)
            .await
            .unwrap()
            .unwrap()
            .pending_connection
            .unwrap()
            .record_id;
        let view = f.action(&pending, action).await;
        assert_eq!(
            view.step,
            if action == "cancel" {
                SetupStep::Cancelled
            } else {
                SetupStep::Owner
            }
        );
        assert_eq!(*f.dependencies.channel_cancellations.lock().unwrap(), [old]);
        f.dependencies.channel_ready.store(true, Ordering::SeqCst);
        let event = f.event(SetupInput::Notification);
        let next = f.advance(&event).await.unwrap();
        assert_eq!(next.step, view.step);
        assert!(next.connection_ref.is_none());
        assert_eq!(f.dependencies.channel_calls.lock().unwrap().len(), 1);
        assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn credential_incarnation_change_cannot_attach_a_ready_connection() {
    let mut f = Fixture::new();
    let pending = connect(&mut f).await;
    f.dependencies
        .channel_incarnation
        .store(2, Ordering::SeqCst);
    f.dependencies.channel_ready.store(true, Ordering::SeqCst);
    let event = f.event(SetupInput::Notification);
    assert!(matches!(f.advance(&event).await, Err(BotError::Forbidden)));
    let stored = f.store.get(&f.actor).await.unwrap().unwrap();
    assert!(stored.connection_ref.is_none());
    assert_eq!(stored.step, SetupStep::ExternalBot);
    // A revoked credential cannot write to the gateway, but can abandon its local request.
    let event = f.event(SetupInput::Text("cancel".into()));
    let cancelled = f.advance(&event).await.unwrap();
    assert_eq!(cancelled.step, SetupStep::Cancelled);
    assert!(f
        .dependencies
        .channel_cancellations
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(f.dependencies.provision_calls.load(Ordering::SeqCst), 0);
    assert_eq!(pending.step, SetupStep::ExternalBot);
}

#[tokio::test]
async fn grant_change_discards_pending_connection_before_login() {
    let mut f = Fixture::new();
    connect(&mut f).await;
    f.dependencies
        .identities
        .lock()
        .unwrap()
        .get_mut(&f.actor.key())
        .unwrap()
        .grant_id = Some("other-grant".into());
    f.dependencies.channel_ready.store(true, Ordering::SeqCst);
    let event = f.event(SetupInput::Notification);
    let reset = f.advance(&event).await.unwrap();
    assert_eq!(reset.step, SetupStep::Login);
    let stored = f.store.get(&f.actor).await.unwrap().unwrap();
    assert!(stored.pending_connection.is_none());
    assert!(stored.connection_ref.is_none());
    assert!(stored.owner.is_none());
    assert_eq!(f.dependencies.channel_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn terminal_connection_can_be_retried_with_a_new_durable_identity() {
    let mut f = Fixture::new();
    let pending = connect(&mut f).await;
    let old = {
        let mut records = f.store.records.lock().unwrap();
        let request = records
            .get_mut(&f.actor.key())
            .unwrap()
            .pending_connection
            .as_mut()
            .unwrap();
        request.status = "failed".into();
        request.setup_url = None;
        request.record_id.clone()
    };
    let retried = f.action(&pending, "register").await;
    let current = f
        .store
        .get(&f.actor)
        .await
        .unwrap()
        .unwrap()
        .pending_connection
        .unwrap();
    assert_ne!(current.record_id, old);
    assert_eq!(retried.connection_status.as_deref(), Some("pending"));
    assert_eq!(*f.dependencies.channel_cancellations.lock().unwrap(), [old]);
}

#[tokio::test]
async fn legacy_review_digest_does_not_gain_a_null_connection_field() {
    let mut f = Fixture::new();
    f.review().await;
    let setup = f.store.get(&f.actor).await.unwrap().unwrap();
    let old = crate::profiles::content_digest(&serde_json::json!({
        "id":setup.id,"grant":setup.grant_id,"owner":setup.owner,"name":setup.draft.name,
        "configuration":setup.configuration,"services":setup.selected_services,
        "external_bot_id":setup.external_bot_id,"admission":setup.admission,
    }))
    .unwrap();
    assert_eq!(setup.plan_digest().unwrap(), old);
}

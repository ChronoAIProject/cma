use super::*;

fn independent(f: &Fixture, enabled: bool) -> Arc<BotService> {
    BotService::new(
        f.store.clone(),
        Arc::new(tests::Profiles),
        "registered".into(),
    )
    .with_effect_reservations(enabled)
}

async fn assert_retained(f: &Fixture) {
    let before = f.bot().await;
    assert!(before.work.as_ref().unwrap().effect.is_some());
    let incompatible = if before.work.as_ref().unwrap().operation == BotOperation::Delete {
        BotOperation::Disable
    } else {
        BotOperation::Delete
    };
    let successor = independent(f, true);
    assert!(successor
        .claim(
            &f.bot_id,
            "successor",
            SystemTime::now() + Duration::from_secs(3600)
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        successor
            .admit(&tests::caller("owner"), &f.bot_id, incompatible)
            .await,
        Err(BotError::Conflict)
    );
    let calls = f.external.0.lock().unwrap().calls.clone();
    BotWorker::new(successor, f.external.clone(), "successor".into())
        .run(&f.bot_id)
        .await
        .unwrap();
    assert_eq!(f.external.0.lock().unwrap().calls, calls);
    assert_eq!(f.bot().await, before);
}

#[tokio::test]
async fn protected_lifecycle_completes_for_legacy_and_connected_channels() {
    for connected in [false, true] {
        let mut request = tests::input();
        if connected {
            request.external_bot_id.clear();
            request.connection_ref = Some("connection-fixture".into());
        }
        let f = Fixture::with_policy(request, true).await;
        f.worker.run(&f.bot_id).await.unwrap();
        assert_eq!(f.bot().await.state, BotState::Active);
        for (op, state) in [
            (BotOperation::Disable, BotState::Disabled),
            (BotOperation::Rotate, BotState::Disabled),
            (BotOperation::Enable, BotState::Active),
            (BotOperation::Rotate, BotState::Active),
            (BotOperation::Delete, BotState::Deleted),
        ] {
            f.admit(op).await;
            f.worker.run(&f.bot_id).await.unwrap();
            let bot = f.bot().await;
            assert_eq!(bot.state, state);
            assert!(bot.work.is_none());
        }
        let remote = f.external.0.lock().unwrap();
        assert!(remote.keys.is_empty() && remote.channel.is_none() && remote.route.is_none());
    }
}

#[tokio::test]
async fn every_unknown_mutation_retains_custody_before_and_after_remote_acceptance() {
    for (operation, steps) in [
        (
            BotOperation::Create,
            vec![
                "mint_agent",
                "mint_route",
                "create_channel",
                "set_callback",
                "create_route",
                "configure_enabled",
            ],
        ),
        (
            BotOperation::Rotate,
            vec![
                "mint_agent",
                "mint_route",
                "set_callback",
                "configure_enabled",
                "set_route_key",
                "revoke",
            ],
        ),
        (BotOperation::Disable, vec!["configure_disabled"]),
        (
            BotOperation::Enable,
            vec!["set_route_key", "configure_enabled"],
        ),
        (
            BotOperation::Delete,
            vec!["delete_channel", "delete_routes", "revoke"],
        ),
    ] {
        for step in steps {
            for after in [false, true] {
                let f = Fixture::fenced().await;
                if operation != BotOperation::Create {
                    f.worker.run(&f.bot_id).await.unwrap();
                    if operation == BotOperation::Enable {
                        f.admit(BotOperation::Disable).await;
                        f.worker.run(&f.bot_id).await.unwrap();
                    }
                    f.admit(operation).await;
                }
                f.fail(step, after);
                assert_eq!(
                    f.worker.run(&f.bot_id).await,
                    Err(BotError::Unavailable),
                    "{operation:?} {step} {after}"
                );
                assert_retained(&f).await;
            }
        }
    }
}

#[tokio::test]
async fn paused_effect_cannot_be_overtaken_and_late_success_publishes_resources() {
    for after in [false, true] {
        let f = Fixture::fenced().await;
        let pause = fixture::MintPause::new(after);
        *f.external.1.lock().unwrap() = Some(pause.clone());
        let worker = BotWorker::new(f.service.clone(), f.external.clone(), "first".into());
        let id = f.bot_id.clone();
        let task = tokio::spawn(async move { worker.run(&id).await });
        tokio::time::timeout(Duration::from_secs(5), pause.entered.notified())
            .await
            .unwrap();
        assert_eq!(f.external.0.lock().unwrap().keys.len(), usize::from(after));
        assert_retained(&f).await;
        pause.release.notify_one();
        task.await.unwrap().unwrap();
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Active);
        assert!(bot.work.is_none());
        assert_eq!(f.external.0.lock().unwrap().keys.len(), 2);
        assert!(bot.resources.credentials.agent_key_id.is_some());
    }
}

#[tokio::test]
async fn cancelled_worker_keeps_accepted_effect_owned_after_expiry() {
    let f = Fixture::fenced().await;
    let pause = fixture::MintPause::new(true);
    *f.external.1.lock().unwrap() = Some(pause.clone());
    let worker = BotWorker::new(f.service.clone(), f.external.clone(), "first".into());
    let id = f.bot_id.clone();
    let task = tokio::spawn(async move { worker.run(&id).await });
    tokio::time::timeout(Duration::from_secs(5), pause.entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_retained(&f).await;
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 1);
}

#[tokio::test]
async fn admission_closure_preserves_protected_settlement_and_legacy_unknown_is_retained() {
    let f = Fixture::fenced().await;
    let off = independent(&f, false);
    let mut owned = off
        .claim(&f.bot_id, "off", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owned.work.as_ref().unwrap().effect_protocol, 1);
    off.reserve_effect(&mut owned, "off", "mint_agent_key")
        .await
        .unwrap();
    assert_retained(&f).await;
    off.settle_effect(&mut owned, "off").await.unwrap();
    assert!(f.bot().await.work.as_ref().unwrap().effect.is_none());
    let legacy = Fixture::new().await;
    legacy
        .service
        .claim(&legacy.bot_id, "old-image", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    assert!(independent(&legacy, true)
        .claim(
            &legacy.bot_id,
            "new-image",
            SystemTime::now() + Duration::from_secs(3600)
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pending_effect_cannot_change_owner_operation_generation_or_identity() {
    let f = Fixture::fenced().await;
    let mut bot = f
        .service
        .claim(&f.bot_id, "first", SystemTime::now())
        .await
        .unwrap()
        .unwrap();
    f.service
        .reserve_effect(&mut bot, "first", "mint_agent_key")
        .await
        .unwrap();
    for change in 0..6 {
        let mut next = bot.clone();
        next.revision += 1;
        let work = next.work.as_mut().unwrap();
        match change {
            0 => work.holder = Some("successor".into()),
            1 => work.operation = BotOperation::Cleanup,
            2 => work.id = uuid::Uuid::new_v4().to_string(),
            3 => work.effect.as_mut().unwrap().id = uuid::Uuid::new_v4().to_string(),
            4 => {
                work.generation += 1;
                next.generation += 1;
            }
            _ => {
                next.work = None;
                next.state = BotState::Failed {
                    reason: "abandoned".into(),
                };
            }
        }
        assert!(f.store.replace(bot.revision, &next).await.is_err());
        assert_eq!(f.bot().await, bot);
    }
    assert_eq!(
        f.service.settle_effect(&mut bot, "other").await,
        Err(BotError::Conflict)
    );
    f.service.settle_effect(&mut bot, "first").await.unwrap();
    let settled = bot.clone();
    assert_eq!(
        f.service.settle_effect(&mut bot, "first").await,
        Err(BotError::Conflict)
    );
    assert_eq!(f.bot().await, settled);
}

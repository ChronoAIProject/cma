use super::*;

#[tokio::test]
async fn disabled_runtime_renews_repeatedly_after_its_external_route_is_removed() {
    let f = Fixture::new().await;
    f.worker.run(&f.bot_id).await.unwrap();
    f.admit(BotOperation::Disable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    let original = f.bot().await;
    {
        let mut remote = f.external.0.lock().unwrap();
        remote.route = None;
        remote.calls.clear();
    }
    let mut previous = original.resources.credentials.clone();
    for _ in 0..2 {
        f.renewal_due().await;
        f.worker.run_due(&f.bot_id).await.unwrap();
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Disabled);
        assert!(bot.work.is_none() && bot.resources.replacement.is_none());
        assert_ne!(bot.resources.credentials, previous);
        assert_eq!(bot.profiles, original.profiles);
        assert_eq!(bot.resources.route_id, original.resources.route_id);
        let channel = bot.resources.channel.as_ref().unwrap();
        let retained = original.resources.channel.as_ref().unwrap();
        assert_eq!(channel.channel_id, retained.channel_id);
        assert_eq!(channel.binding_id, retained.binding_id);
        let remote = f.external.0.lock().unwrap();
        assert_eq!(remote.keys.len(), 2);
        assert!(remote.route.is_none() && !remote.enabled);
        assert_eq!(
            remote.provider_hash,
            bot.resources.credentials.agent_key_hash
        );
        previous = bot.resources.credentials;
    }
    let remote = f.external.0.lock().unwrap();
    assert!(remote.calls.iter().all(|call| matches!(
        call.as_str(),
        "channel"
            | "mint_agent"
            | "mint_route"
            | "set_callback"
            | "configure_disabled"
            | "active_route_key"
            | "keys"
            | "revoke"
    )));
}

#[tokio::test]
async fn interrupted_disabled_renewal_never_restores_or_recreates_a_removed_route() {
    for after in [false, true] {
        let f = Fixture::new().await;
        f.worker.run(&f.bot_id).await.unwrap();
        f.admit(BotOperation::Disable).await;
        f.worker.run(&f.bot_id).await.unwrap();
        {
            let mut remote = f.external.0.lock().unwrap();
            remote.route = None;
            remote.calls.clear();
        }
        f.admit(BotOperation::Rotate).await;
        f.fail("configure_disabled", after);
        assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
        f.ready().await;
        f.worker.run(&f.bot_id).await.unwrap();
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Disabled);
        assert!(bot.work.is_none() && bot.resources.replacement.is_none());
        let remote = f.external.0.lock().unwrap();
        assert_eq!(remote.keys.len(), 2);
        assert!(remote.route.is_none() && !remote.enabled);
        assert!(!remote.calls.iter().any(|call| matches!(
            call.as_str(),
            "set_route_key" | "create_route" | "delete_routes" | "delete_channel"
        )));
    }
}

#[tokio::test]
async fn disabled_renewal_preserves_only_the_bound_key_until_enable_reconciles_it() {
    let f = Fixture::new().await;
    f.worker.run(&f.bot_id).await.unwrap();
    let original_route = f.external.0.lock().unwrap().route.clone().unwrap();
    f.admit(BotOperation::Disable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    for _ in 0..2 {
        f.renewal_due().await;
        f.worker.run_due(&f.bot_id).await.unwrap();
        let remote = f.external.0.lock().unwrap();
        assert_eq!(remote.keys.len(), 3);
        assert_eq!(remote.route, Some(original_route.clone()));
        assert!(!remote.enabled);
    }
    let current = f.bot().await.resources.credentials;
    f.external.0.lock().unwrap().calls.clear();
    f.admit(BotOperation::Enable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await.state, BotState::Active);
    let remote = f.external.0.lock().unwrap();
    assert!(remote.enabled);
    assert_eq!(remote.keys.len(), 2);
    assert_eq!(
        remote.route.as_ref().unwrap().1,
        current.route_key_id.unwrap()
    );
    let route_update = remote
        .calls
        .iter()
        .position(|c| c == "set_route_key")
        .unwrap();
    let admission = remote
        .calls
        .iter()
        .position(|c| c == "configure_enabled")
        .unwrap();
    assert!(route_update < admission);
}

#[tokio::test]
async fn enable_fails_closed_when_the_retained_route_is_unavailable() {
    for removed in [true, false] {
        let f = Fixture::new().await;
        f.worker.run(&f.bot_id).await.unwrap();
        f.admit(BotOperation::Disable).await;
        f.worker.run(&f.bot_id).await.unwrap();
        {
            let mut remote = f.external.0.lock().unwrap();
            if removed {
                remote.route = None;
            }
            remote.calls.clear();
        }
        if !removed {
            f.fail("set_route_key", false);
        }
        f.admit(BotOperation::Enable).await;
        assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Disabled);
        assert_eq!(bot.work.unwrap().operation, BotOperation::Enable);
        let remote = f.external.0.lock().unwrap();
        assert!(!remote.enabled);
        assert_eq!(remote.calls, ["channel", "set_route_key"]);
    }
}

#[tokio::test]
async fn disabled_renewal_retries_when_route_inventory_is_uncertain() {
    let f = Fixture::new().await;
    f.worker.run(&f.bot_id).await.unwrap();
    f.admit(BotOperation::Disable).await;
    f.worker.run(&f.bot_id).await.unwrap();
    f.external.0.lock().unwrap().calls.clear();
    f.admit(BotOperation::Rotate).await;
    f.fail("active_route_key", false);
    assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
    let bot = f.bot().await;
    assert_eq!(bot.state, BotState::Disabled);
    assert_eq!(bot.work.unwrap().operation, BotOperation::Rotate);
    assert!(!f
        .external
        .0
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|c| c == "revoke"));
    f.ready().await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert!(f.bot().await.work.is_none());
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 3);
}

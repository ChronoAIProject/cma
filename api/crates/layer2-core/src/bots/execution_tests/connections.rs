use super::*;

async fn fixture() -> Fixture {
    let mut request = tests::input();
    request.external_bot_id.clear();
    request.connection_ref = Some(uuid::Uuid::new_v4().to_string());
    Fixture::with_request(request).await
}

fn no_route_effects(f: &Fixture) {
    let remote = f.external.0.lock().unwrap();
    assert!(remote.calls.iter().all(|call| !matches!(
        call.as_str(),
        "mint_route"
            | "set_callback"
            | "create_route"
            | "set_route_key"
            | "active_route_key"
            | "delete_routes"
    )));
    assert!(remote.route.is_none());
    assert!(remote
        .keys
        .values()
        .all(|(role, _)| *role == BotKeyRole::Agent));
}

#[tokio::test]
async fn gateway_connection_lifecycle_uses_only_agent_credentials() {
    let f = fixture().await;
    f.worker.run(&f.bot_id).await.unwrap();
    let first = f.bot().await;
    assert_eq!(first.state, BotState::Active);
    assert!(first.resources.credentials.route_key_id.is_none());
    assert_eq!(f.external.0.lock().unwrap().keys.len(), 1);
    for operation in [
        BotOperation::Disable,
        BotOperation::Rotate,
        BotOperation::Enable,
        BotOperation::Rotate,
    ] {
        f.admit(operation).await;
        f.worker.run(&f.bot_id).await.unwrap();
        let current = f.bot().await;
        assert!(current.work.is_none());
        assert!(current.resources.credentials.route_key_id.is_none());
        assert_eq!(f.external.0.lock().unwrap().keys.len(), 1);
        no_route_effects(&f);
    }
    assert_ne!(
        first.resources.credentials.agent_key_id,
        f.bot().await.resources.credentials.agent_key_id
    );
    f.admit(BotOperation::Delete).await;
    f.worker.run(&f.bot_id).await.unwrap();
    assert_eq!(f.bot().await.state, BotState::Deleted);
    assert!(f.external.0.lock().unwrap().keys.is_empty());
    no_route_effects(&f);
}

#[tokio::test]
async fn connection_rotation_recovers_lost_channel_or_revoke_response() {
    for (step, after) in [
        ("mint_agent", true),
        ("configure_enabled", false),
        ("configure_enabled", true),
        ("revoke", true),
    ] {
        let f = fixture().await;
        f.worker.run(&f.bot_id).await.unwrap();
        f.admit(BotOperation::Rotate).await;
        f.fail(step, after);
        assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
        f.ready().await;
        f.worker.run(&f.bot_id).await.unwrap();
        let bot = f.bot().await;
        assert_eq!(bot.state, BotState::Active);
        assert!(bot.work.is_none() && bot.resources.replacement.is_none());
        assert_eq!(f.external.0.lock().unwrap().keys.len(), 1);
        no_route_effects(&f);
    }
}

#[tokio::test]
async fn failed_connection_creation_cleans_only_owned_channel_and_agent_key() {
    for (step, after) in [("mint_agent", true), ("create_channel", true)] {
        let f = fixture().await;
        f.fail(step, after);
        assert_eq!(f.worker.run(&f.bot_id).await, Err(BotError::Unavailable));
        f.ready().await;
        f.worker.run(&f.bot_id).await.unwrap();
        assert!(matches!(f.bot().await.state, BotState::Failed { .. }));
        assert!(f.external.0.lock().unwrap().keys.is_empty());
        assert!(f.external.0.lock().unwrap().channel.is_none());
        no_route_effects(&f);
    }
}

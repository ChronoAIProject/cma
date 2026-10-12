use super::*;

#[tokio::test]
async fn disabled_route_inventory_requires_exact_retained_lineage_and_complete_results() {
    let server = Server::start().await;
    let client = nyxid::NyxidClient {
        http: reqwest::Client::new(),
        base: server.base.clone(),
    };
    let mut bot = bot().await;
    bot.resources.route_id = Some("retained-route".into());
    let owned = ["retained-key".into(), "replacement-key".into()];
    let route = json!({"id":"retained-route","channel_bot_id":bot.request.external_bot_id,
        "platform":"telegram","agent_api_key_id":"retained-key","platform_conversation_id":"*",
        "default_agent":true,"is_active":true});
    let read = || client.active_route_key("owner-token", &bot, &owned);
    assert_eq!(read().await.unwrap(), None);
    server
        .identity
        .lock()
        .unwrap()
        .routes
        .insert("retained-route".into(), route.clone());
    assert_eq!(read().await.unwrap().as_deref(), Some("retained-key"));
    for (field, invalid) in [
        ("channel_bot_id", json!("another-bot")),
        ("agent_api_key_id", json!("another-key")),
        ("platform", json!("discord")),
        ("platform_conversation_id", json!("another-chat")),
        ("default_agent", json!(false)),
        ("is_active", json!(false)),
    ] {
        let mut changed = route.clone();
        changed[field] = invalid;
        server
            .identity
            .lock()
            .unwrap()
            .routes
            .insert("retained-route".into(), changed);
        assert!(read().await.is_err(), "{field}");
    }
    server
        .identity
        .lock()
        .unwrap()
        .routes
        .insert("retained-route".into(), route.clone());
    for (bot_id, key) in [
        (bot.request.external_bot_id.as_str(), "unowned-key"),
        ("other-bot", "retained-key"),
    ] {
        let mut conflicting = route.clone();
        conflicting["id"] = "other-route".into();
        conflicting["channel_bot_id"] = bot_id.into();
        conflicting["agent_api_key_id"] = key.into();
        server
            .identity
            .lock()
            .unwrap()
            .routes
            .insert("other-route".into(), conflicting);
        assert_eq!(read().await, Err(BotError::Forbidden));
    }
    {
        let mut identity = server.identity.lock().unwrap();
        identity.routes.remove("other-route");
        identity.routes_total = Some(2);
    }
    assert_eq!(read().await, Err(BotError::Unavailable));
    {
        let mut identity = server.identity.lock().unwrap();
        identity.routes_total = None;
        identity.routes.insert("duplicate".into(), route);
    }
    assert_eq!(read().await, Err(BotError::Unavailable));
    assert!(server.identity.lock().unwrap().calls.is_empty());
}

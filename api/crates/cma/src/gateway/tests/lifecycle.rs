use super::*;
struct Tokens;
#[async_trait::async_trait]
impl OwnerTokens for Tokens {
    async fn bot_owner_token(
        &self,
        owner: &SubjectId,
    ) -> Result<crate::auth::persistence::SecretValue, BotError> {
        assert_eq!(owner.as_str(), "owner");
        Ok(crate::auth::persistence::SecretValue::new(
            "owner-token".into(),
        ))
    }
}
struct Profiles(BotProfiles);
#[async_trait::async_trait]
impl BotProfileResolver for Profiles {
    async fn resolve(&self, _: &SubjectId, _: &CreateBot) -> Result<BotProfiles, BotError> {
        Ok(self.0.clone())
    }
}
pub(super) fn external(server: &Server, gateway: &Gateway) -> Arc<GatewayBots> {
    external_at(server, &gateway.base, &gateway.base)
}
pub(super) fn external_at(server: &Server, base: &str, public: &str) -> Arc<GatewayBots> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    Arc::new(GatewayBots {
        auth: Arc::new(Tokens),
        gateway: client::GatewayClient::new(
            http.clone(),
            &GatewayConfig {
                binding_preparation_enabled: false,
                base_url: base.into(),
                public_url: public.into(),
                public_api_url: public.into(),
            },
            "https://nyx.example".into(),
        )
        .unwrap(),
        nyxid: nyxid::NyxidClient {
            http: http.clone(),
            base: server.base.clone(),
        },
        connections: connections::ChannelConnections::new(http.clone(), base, public).unwrap(),
        issuer: KeyIssuer::new(http, &server.base, "test"),
        public: public.into(),
    })
}
fn owner() -> layer2_core::Caller {
    layer2_core::Caller::new(
        SubjectId::new("owner"),
        layer2_core::Authorization::new([layer2_core::Capability::ManageOwnBots]),
    )
}
pub(super) async fn setup(
    server: &Server,
    gateway: &Gateway,
) -> (Arc<BotService>, Arc<InMemoryBotStore>, BotWorker, Bot) {
    let bot = bot().await;
    let store = Arc::new(InMemoryBotStore::default());
    store.insert(&bot).await.unwrap();
    let service = BotService::new(
        store.clone(),
        Arc::new(Profiles(bot.profiles.clone())),
        "cma_codex".into(),
    );
    let worker = BotWorker::new(service.clone(), external(server, gateway), "worker".into());
    (service, store, worker, bot)
}
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn production_effects_create_disable_rotate_and_delete_through_compiled_gateway() {
    let server = Server::start().await;
    let mut gateway = Gateway::start(&server).await;
    let (service, _, worker, initial) = setup(&server, &gateway).await;
    worker.run(&initial.bot_id).await.unwrap();
    let active = service.get(&owner(), &initial.bot_id).await.unwrap();
    assert_eq!(active.state, BotState::Active);
    assert_eq!(
        server.identity.lock().unwrap().calls,
        ["mint_agent", "mint_route", "callback", "create_route"]
    );
    assert_eq!(server.identity.lock().unwrap().keys.len(), 2);
    assert_eq!(server.identity.lock().unwrap().routes.len(), 1);
    gateway.restart(&server).await;
    for operation in [
        BotOperation::Disable,
        BotOperation::Rotate,
        BotOperation::Enable,
    ] {
        service
            .admit(&owner(), &initial.bot_id, operation)
            .await
            .unwrap();
        worker.run(&initial.bot_id).await.unwrap();
        let bot = service.get(&owner(), &initial.bot_id).await.unwrap();
        assert_eq!(
            bot.state,
            if operation == BotOperation::Enable {
                BotState::Active
            } else {
                BotState::Disabled
            }
        );
        assert_eq!(
            server.identity.lock().unwrap().keys.len(),
            if operation == BotOperation::Rotate {
                3
            } else {
                2
            }
        );
        assert_eq!(
            bot.resources.channel.as_ref().unwrap().channel_id,
            active.resources.channel.as_ref().unwrap().channel_id
        );
    }
    service
        .admit(&owner(), &initial.bot_id, BotOperation::Delete)
        .await
        .unwrap();
    worker.run(&initial.bot_id).await.unwrap();
    assert_eq!(
        service.get(&owner(), &initial.bot_id).await,
        Err(BotError::NotFound)
    );
    assert!(server.identity.lock().unwrap().keys.is_empty());
    assert!(server.identity.lock().unwrap().routes.is_empty());
    assert!(server.remote.records.lock().unwrap().is_empty());
    assert!(server.remote.provider_keys.lock().unwrap().is_empty());
}
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn disabled_runtime_rotates_after_route_removal_and_cannot_enable_without_a_route() {
    let server = Server::start().await;
    let gateway = Gateway::start(&server).await;
    let (service, _, worker, initial) = setup(&server, &gateway).await;
    worker.run(&initial.bot_id).await.unwrap();
    service
        .admit(&owner(), &initial.bot_id, BotOperation::Disable)
        .await
        .unwrap();
    worker.run(&initial.bot_id).await.unwrap();
    let disabled = service.get(&owner(), &initial.bot_id).await.unwrap();
    let original_channel = disabled.resources.channel.unwrap();
    let mut previous = disabled.resources.credentials;
    {
        let mut identity = server.identity.lock().unwrap();
        identity.routes.clear();
        identity.calls.clear();
    }
    for _ in 0..2 {
        service
            .admit(&owner(), &initial.bot_id, BotOperation::Rotate)
            .await
            .unwrap();
        worker.run(&initial.bot_id).await.unwrap();
        let bot = service.get(&owner(), &initial.bot_id).await.unwrap();
        assert_eq!(bot.state, BotState::Disabled);
        assert!(bot.work.is_none());
        assert_ne!(bot.resources.credentials, previous);
        previous = bot.resources.credentials;
        let channel = bot.resources.channel.unwrap();
        assert_eq!(channel.channel_id, original_channel.channel_id);
        assert_eq!(channel.binding_id, original_channel.binding_id);
        assert_eq!(server.identity.lock().unwrap().keys.len(), 2);
        let persisted: Value = reqwest::Client::new()
            .get(format!(
                "{}/v1/channel-records/{}",
                gateway.base, initial.record_id
            ))
            .bearer_auth("owner-token")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(persisted["channel_id"], original_channel.channel_id);
        assert_eq!(
            persisted["definition"]["sources"][0]["admission"]["senders"]["type"],
            "deny"
        );
        assert_eq!(server.remote.provider_keys.lock().unwrap().len(), 1);
    }
    service
        .admit(&owner(), &initial.bot_id, BotOperation::Enable)
        .await
        .unwrap();
    assert_eq!(
        worker.run(&initial.bot_id).await,
        Err(BotError::Unavailable)
    );
    assert_eq!(
        service.get(&owner(), &initial.bot_id).await.unwrap().state,
        BotState::Disabled
    );
    let channel: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/channel-records/{}",
            gateway.base, initial.record_id
        ))
        .bearer_auth("owner-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        channel["definition"]["sources"][0]["admission"],
        json!({"type":"scoped","senders":{"type":"deny"},"chats":{"type":"deny"},"groups":"deny"})
    );
    let identity = server.identity.lock().unwrap();
    assert!(identity.routes.is_empty());
    assert!(identity.calls.iter().all(|call| matches!(
        call.as_str(),
        "mint_agent" | "mint_route" | "callback" | "revoke"
    )));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn unknown_nyxid_creation_outcomes_clean_up_owned_effects_after_gateway_restart() {
    for failed_step in [
        "mint_agent",
        "mint_route",
        "callback",
        "create_route",
        "changed_expiry",
    ] {
        let server = Server::start().await;
        let mut gateway = Gateway::start(&server).await;
        let (service, store, worker, initial) = setup(&server, &gateway).await;
        server.identity.lock().unwrap().fail_after = Some(failed_step);
        if failed_step == "changed_expiry" {
            server.identity.lock().unwrap().expiry_suffix = Some(".000000001+00:00");
        }
        assert!(worker.run(&initial.bot_id).await.is_err());
        if failed_step == "changed_expiry" {
            let state = server.identity.lock().unwrap();
            assert_eq!(state.calls, ["mint_agent"]);
            assert_eq!(state.keys.len(), 1);
        }
        gateway.restart(&server).await;
        let mut bot = store.get(&initial.bot_id).await.unwrap().unwrap();
        let prior = bot.revision;
        bot.revision += 1;
        bot.work.as_mut().unwrap().expires_at = Some(layer2_core::rfc3339_at(
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        ));
        store.replace(prior, &bot).await.unwrap();
        worker.run(&initial.bot_id).await.unwrap();
        let bot = service.get(&owner(), &initial.bot_id).await.unwrap();
        assert!(matches!(bot.state, BotState::Failed { .. }));
        assert!(bot.work.is_none());
        assert!(
            server.identity.lock().unwrap().keys.is_empty(),
            "{failed_step}"
        );
        assert!(
            server.identity.lock().unwrap().routes.is_empty(),
            "{failed_step}"
        );
        assert!(
            server.remote.records.lock().unwrap().is_empty(),
            "{failed_step}"
        );
        assert!(
            server.remote.provider_keys.lock().unwrap().is_empty(),
            "{failed_step}"
        );
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn required_profile_refusal_cleans_up_instead_of_activating_a_partial_bot() {
    let server = Server::start().await;
    let gateway = Gateway::start(&server).await;
    let (service, store, worker, initial) = setup(&server, &gateway).await;
    *server.remote.refusal.lock().unwrap() = Some("model.service".into());
    assert!(worker.run(&initial.bot_id).await.is_err());
    let mut bot = store.get(&initial.bot_id).await.unwrap().unwrap();
    assert!(matches!(bot.state, BotState::Failed { .. }));
    let prior = bot.revision;
    bot.revision += 1;
    bot.work.as_mut().unwrap().expires_at = Some(layer2_core::rfc3339_at(
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1),
    ));
    store.replace(prior, &bot).await.unwrap();
    worker.run(&initial.bot_id).await.unwrap();
    assert!(service
        .get(&owner(), &initial.bot_id)
        .await
        .unwrap()
        .work
        .is_none());
    assert!(server.identity.lock().unwrap().keys.is_empty());
    assert!(server.identity.lock().unwrap().routes.is_empty());
    assert!(server.remote.records.lock().unwrap().is_empty());
    assert!(server.remote.provider_keys.lock().unwrap().is_empty());
}

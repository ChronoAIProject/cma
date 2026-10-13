//! Browser interoperability over real consumer routes, workers, Gateway and Mongo.
//! Identity, image catalogue, profile materialization, provider and platform APIs are local fakes.
use super::*;
use axum::{body::Body, http::Request, middleware::Next};
use std::path::Path;

struct Profiles(BotProfiles);
#[async_trait::async_trait]
impl BotProfileResolver for Profiles {
    async fn resolve(&self, _: &SubjectId, _: &CreateBot) -> Result<BotProfiles, BotError> {
        panic!("the consumer browser journey must use inline Agent configuration");
    }
}
#[async_trait::async_trait]
impl layer2_core::bots::BotInlineProfileResolver for Profiles {
    async fn resolve_inline(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
        _: Option<&str>,
        key: &str,
    ) -> Result<BotProfiles, BotError> {
        assert_eq!(owner, &SubjectId::new("owner"));
        assert!(!key.is_empty());
        assert!(request.image.is_none());
        assert!(request.repository.is_none());
        assert!(request.sandbox_profile.is_none());
        assert!(request.workspace_profile.is_none());
        let agent = request
            .inline_agent
            .as_ref()
            .expect("inline Agent required");
        assert_eq!(agent.model.as_deref(), Some("gpt-5"));
        assert_eq!(agent.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(agent.permission_mode.as_deref(), Some("guarded"));
        assert_eq!(
            agent.system_prompt.as_deref(),
            Some("Respond with concise updates.")
        );
        assert!(agent.skills.is_empty());
        assert!(agent.permissions.services.is_empty());
        Ok(self.0.clone())
    }
}

#[tokio::test]
#[ignore = "requires authenticated service Mongo, built web/, Playwright and CMAEG migrations"]
async fn dashboard_creates_tests_and_deletes_a_gateway_connection_consumer() {
    let remote = Server::start().await;
    let gateway = consumer_fixture::ConsumerGateway::start(&remote).await;
    let template = bot().await;
    let store = Arc::new(InMemoryBotStore::default());
    let profiles = Arc::new(Profiles(template.profiles));
    let service = BotService::new(store.clone(), profiles.clone(), "cma_codex".into())
        .with_inline_profiles(profiles);
    let external = lifecycle::external_at(&remote, &gateway.base, consumer_fixture::PUBLIC);
    let worker = BotWorker::new(service.clone(), external.clone(), "web-fixture".into());
    let router = layer1_public::rest::bot_router(BotControl::new(service, external)).layer(
        axum::middleware::from_fn(|mut request: Request<Body>, next: Next| async move {
            request
                .extensions_mut()
                .insert(layer1_public::AuthenticatedSubject(SubjectId::new("owner")));
            request
                .extensions_mut()
                .insert(layer2_core::Authorization::new([
                    layer2_core::Capability::ManageOwnBots,
                ]));
            next.run(request).await
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let pending = store.clone();
    let execution = tokio::spawn(async move {
        loop {
            for bot in pending.pending(None, 100).await.unwrap() {
                if bot.work.is_some() {
                    worker.run(&bot.bot_id).await.unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut browser = tokio::process::Command::new("node")
        .current_dir(root.join("web"))
        .args([
            "node_modules/@playwright/test/cli.js",
            "test",
            "--config",
            "e2e/bots.config.ts",
        ])
        .env("CMA_BOT_FIXTURE_ROOT", origin)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(180), browser.wait()).await;
    server.abort();
    assert!(
        !execution.is_finished(),
        "Bot worker failed during the browser journey"
    );
    execution.abort();
    assert!(
        result.expect("browser timed out").unwrap().success(),
        "browser journey failed"
    );
    assert!(store
        .list(&SubjectId::new("owner"), None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(remote.identity.lock().unwrap().keys.is_empty());
    assert!(remote.identity.lock().unwrap().routes.is_empty());
    assert!(remote.remote.records.lock().unwrap().is_empty());
    assert!(remote.remote.provider_keys.lock().unwrap().is_empty());
    {
        let platform = gateway.platform.lock().unwrap();
        assert!(platform.webhook.is_empty());
        assert!(platform.calls.iter().any(|call| call == "setWebhook"));
        assert!(platform.calls.iter().any(|call| call == "deleteWebhook"));
    }
    let (connection, _) = gateway.store.connection(&gateway.connection).await.unwrap();
    assert!(connection.channel_record.is_none());
}

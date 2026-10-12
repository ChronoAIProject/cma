//! Opt-in browser interoperability: real CMA Bot routes/worker and compiled CMAEG.
//! Identity, profile catalogue and provider execution are disposable local fakes.
use super::*;
use axum::{body::Body, http::Request, middleware::Next};
use std::path::Path;

struct Profiles(BotProfiles);
#[async_trait::async_trait]
impl BotProfileResolver for Profiles {
    async fn resolve(&self, _: &SubjectId, request: &CreateBot) -> Result<BotProfiles, BotError> {
        let reference = &request.profile;
        assert_eq!(reference.id, self.0.agent.profile_id);
        assert_eq!(reference.revision, self.0.agent.revision);
        Ok(self.0.clone())
    }
}

#[tokio::test]
#[ignore = "requires authenticated service Mongo, built web/, Playwright and compiled CMAEG"]
async fn dashboard_creates_tests_and_deletes_through_real_cma_and_compiled_gateway() {
    let remote = Server::start().await;
    let gateway = Gateway::start(&remote).await;
    let template = bot().await;
    let store = Arc::new(InMemoryBotStore::default());
    let service = BotService::new(
        store.clone(),
        Arc::new(Profiles(template.profiles)),
        "cma_codex".into(),
    );
    let external = lifecycle::external(&remote, &gateway);
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
}

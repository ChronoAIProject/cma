//! Redemption must survive independent gateway instances sharing only records.
use super::{
    download_support::{download, redefine},
    media_links::{mint_rest, native_discord_app, native_whatsapp_app, photo},
    support::App,
};
use cmaeg::{http, tools::ToolsState};
use std::sync::atomic::Ordering;
use tokio::task::JoinHandle;

struct Replica {
    base: String,
    server: JoinHandle<()>,
}
impl Drop for Replica {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Replica {
    async fn new(state: &ToolsState) -> Self {
        let mut state = state.clone();
        state.downloads = Default::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, http::media::router(state))
                .await
                .unwrap();
        });
        Self { base, server }
    }
    async fn download(&self, app: &App, token: &str) -> reqwest::Response {
        app.fixture
            .client
            .get(format!("{}/v1/media/content", self.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn native_media_consumption_survives_replica_changes_and_restart() {
    for setup in [
        native_discord_app(),
        native_whatsapp_app(false),
        native_whatsapp_app(true),
    ] {
        let app = App::with_media(setup).await;
        let view = mint_rest(&app, "0").await;
        let token = view["download"]["authorization"]["token"].as_str().unwrap();
        assert_eq!(download(&app, token).await.bytes().await.unwrap(), photo());
        assert_eq!(download(&app, token).await.status(), 410);
        for _ in 0..2 {
            let replica = Replica::new(&app.state).await;
            assert_eq!(replica.download(&app, token).await.status(), 410);
        }
    }
}

#[tokio::test]
async fn simultaneous_native_media_redemption_on_replicas_has_one_winner() {
    for setup in [
        native_discord_app(),
        native_whatsapp_app(false),
        native_whatsapp_app(true),
    ] {
        let app = App::with_media(setup).await;
        let replica = Replica::new(&app.state).await;
        let view = mint_rest(&app, "0").await;
        let token = view["download"]["authorization"]["token"].as_str().unwrap();
        let (a, b) = tokio::join!(download(&app, token), replica.download(&app, token));
        let mut statuses = vec![a.status().as_u16(), b.status().as_u16()];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 410]);
        for response in [a, b] {
            if response.status() == 200 {
                assert_eq!(response.bytes().await.unwrap(), photo());
            }
        }
    }
}

#[tokio::test]
async fn native_media_storage_failure_or_lost_claim_response_releases_no_bytes() {
    for persisted in [false, true] {
        for setup in [
            native_discord_app(),
            native_whatsapp_app(false),
            native_whatsapp_app(true),
        ] {
            let source = setup.fakes.source.clone();
            let app = App::with_media(setup).await;
            let view = mint_rest(&app, "0").await;
            let token = view["download"]["authorization"]["token"].as_str().unwrap();
            app.fixture
                .backend
                .interaction_write_status
                .store(if persisted { 200 } else { 503 }, Ordering::SeqCst);
            app.fixture
                .backend
                .interaction_lose_response
                .store(persisted, Ordering::SeqCst);
            let response = download(&app, token).await;
            assert_eq!(response.status(), 503);
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap()["detail"],
                "media_redemption_unavailable"
            );
            assert_eq!(source.calls(), 0);
            app.fixture
                .backend
                .interaction_write_status
                .store(200, Ordering::SeqCst);
            app.fixture
                .backend
                .interaction_lose_response
                .store(false, Ordering::SeqCst);
            let replica = Replica::new(&app.state).await;
            let response = replica.download(&app, token).await;
            assert_eq!(response.status(), if persisted { 410 } else { 200 });
            if !persisted {
                assert_eq!(response.bytes().await.unwrap(), photo());
            }
        }
    }
}

#[tokio::test]
async fn refused_native_media_redemption_stays_consumed_on_another_replica() {
    let setup = native_discord_app();
    let source = setup.fakes.source.clone();
    let app = App::with_media(setup).await;
    let view = mint_rest(&app, "0").await;
    let token = view["download"]["authorization"]["token"].as_str().unwrap();
    let original = app.fixture.backend.blob.lock().unwrap().clone();
    redefine(&app, |definition| {
        definition
            .sources
            .retain(|source| !matches!(source, cmaeg_core::channel::Source::Discord { .. }))
    })
    .await;
    assert_eq!(download(&app, token).await.status(), 403);
    *app.fixture.backend.blob.lock().unwrap() = original;
    let replica = Replica::new(&app.state).await;
    assert_eq!(replica.download(&app, token).await.status(), 410);
    assert_eq!(source.calls(), 0);
}

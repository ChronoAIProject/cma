//! Current authority cannot be supplied by another replica's stale channel cache.
use super::{
    download_support::download,
    media_links::{mint_rest, native_discord_app},
    support::App,
};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn native_media_current_authority_outage_fails_closed_despite_warm_cache() {
    let setup = native_discord_app();
    let source = setup.fakes.source.clone();
    let app = App::with_media(setup).await;
    let view = mint_rest(&app, "0").await;
    let token = view["download"]["authorization"]["token"].as_str().unwrap();
    app.fixture
        .backend
        .record_status
        .store(503, Ordering::SeqCst);
    assert_eq!(download(&app, token).await.status(), 503);
    assert_eq!(source.calls(), 0);
}

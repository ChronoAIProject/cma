//! I14: mutations revoke every cached alias and every pending old restore.
use super::app::{GoldenApp, WEBHOOK_SECRET};
use cmaeg::channels::resolver::ResolveError;
use cmaeg_core::ids::ChannelId;
use serde_json::{json, Value};
use std::{sync::atomic::Ordering, time::Duration};

const ROTATED_WEBHOOK: &str = "rotated-webhook-golden";

async fn channel(app: &GoldenApp) -> (String, String) {
    let original = app
        .create(
            json!([{"type":"telegram","bot_id":"77","bot_username":"golden_bot",
                "admission":{"type":"all"}}]),
            "conversation_and_sender",
            json!([{"type":"source"}]),
        )
        .await;
    let pointer = ChannelId::new(&original)
        .unwrap()
        .open(&app.state.master_keys)
        .unwrap()
        .payload;
    // Recovery now retains the original identity. Independently sealing the
    // same validated pointer exercises older recovered ids without changing
    // today's read-only recoverChannel contract.
    let alias = pointer.seal_id(&app.state.master_keys).unwrap().to_string();
    assert_ne!(original, alias);
    (original, alias)
}

async fn update(app: &GoldenApp, id: &str, expected: u64, secret: &str) -> reqwest::Response {
    let current = app.state.resolver.resolve_current(id).await.unwrap();
    let definition = &current.definition;
    app.client
        .put(format!("{}/v1/channels/{id}", app.base))
        .bearer_auth("token-alice")
        .json(&json!({
            "schema_version":1, "expected_version":expected, "name":definition.name,
            "profile":definition.profile, "provider":{"slug":definition.provider.slug},
            "sources":definition.sources, "partition":definition.partition,
            "reply":definition.reply, "credentials":{"telegram_webhook_secret":secret}
        }))
        .send()
        .await
        .unwrap()
}

fn event() -> Value {
    json!({"update_id":101,"message":{"message_id":5,"date":super::relay_support::now()-5,
        "chat":{"id":42,"type":"private"},
        "from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"hi"}})
}

#[tokio::test]
async fn rest_update_through_either_alias_revokes_opposite_alias_ingress() {
    for use_alias in [false, true] {
        let mut app = GoldenApp::start().await;
        let (original, alias) = channel(&app).await;
        let resolver = &app.state.resolver;
        let version = resolver.resolve(&original).await.unwrap().version;
        resolver.resolve(&alias).await.unwrap();
        assert_eq!(resolver.cache_len(), 2);
        let (changed, opposite) = if use_alias {
            (&alias, &original)
        } else {
            (&original, &alias)
        };
        let response = update(&app, changed, version, ROTATED_WEBHOOK).await;
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
        assert_eq!(resolver.cache_len(), 0);
        let response = app.telegram(opposite, &event()).await;
        assert_eq!(response.status(), 401);
        let response = app
            .client
            .post(format!("{}/v1/telegram/{opposite}", app.base))
            .header("x-telegram-bot-api-secret-token", ROTATED_WEBHOOK)
            .json(&event())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 202, "{}", response.text().await.unwrap());
        app.drain().await;
    }
}

#[tokio::test]
async fn old_restore_snapshot_cannot_publish_after_rest_mutation() {
    let mut app = GoldenApp::start().await;
    let (original, alias) = channel(&app).await;
    let resolver = app.state.resolver.clone();
    let version = resolver.resolve(&original).await.unwrap().version;
    let (captured, release) = app.cma.pause_next_get();
    let pending = tokio::spawn({
        let resolver = resolver.clone();
        let alias = alias.clone();
        async move { resolver.resolve(&alias).await }
    });
    tokio::time::timeout(Duration::from_secs(2), captured)
        .await
        .unwrap()
        .unwrap();
    let response = update(&app, &original, version, ROTATED_WEBHOOK).await;
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert_eq!(resolver.cache_len(), 0);
    release.send(()).unwrap();
    let restored = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(restored.version, version + 1);
    assert_eq!(
        restored
            .definition
            .secrets
            .telegram_webhook_secret
            .as_ref()
            .unwrap()
            .as_str(),
        ROTATED_WEBHOOK
    );
    assert_eq!(app.telegram(&alias, &event()).await.status(), 401);
    app.drain().await;
}

#[tokio::test]
async fn negative_alias_is_cleared_without_evicting_an_unrelated_record() {
    let mut app = GoldenApp::start().await;
    let (id, _) = channel(&app).await;
    let unrelated = app.state.resolver.resolve(&id).await.unwrap();
    let missing = cmaeg::channels::definition::channel_pointer(
        &app.state.cma_base_url,
        &cmaeg_core::ids::RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap(),
    )
    .unwrap();
    let alias = missing.seal_id(&app.state.master_keys).unwrap();
    assert!(matches!(
        app.state.resolver.resolve(alias.as_str()).await,
        Err(ResolveError::NotFound)
    ));
    assert_eq!(app.state.resolver.cache_len(), 2);
    app.state
        .resolver
        .invalidate_record(&missing.record_id, Some(alias.as_str()));
    assert_eq!(app.state.resolver.cache_len(), 1);
    let before = app.cma.gets.load(Ordering::SeqCst);
    let still_cached = app.state.resolver.resolve(&id).await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&unrelated, &still_cached));
    assert_eq!(app.cma.gets.load(Ordering::SeqCst), before);
    assert!(matches!(
        app.state.resolver.resolve(alias.as_str()).await,
        Err(ResolveError::NotFound)
    ));
    assert_eq!(app.cma.gets.load(Ordering::SeqCst), before + 1);
    app.drain().await;
}

#[tokio::test]
async fn a_failed_cas_never_publishes_the_unpersisted_definition() {
    let mut app = GoldenApp::start().await;
    let (id, alias) = channel(&app).await;
    let version = app.state.resolver.resolve(&id).await.unwrap().version;
    app.state.resolver.resolve(&alias).await.unwrap();
    app.cma.put_conflicts.store(1, Ordering::SeqCst);
    assert_eq!(
        update(&app, &id, version, ROTATED_WEBHOOK).await.status(),
        409
    );
    assert_eq!(app.state.resolver.cache_len(), 0);
    let restored = app.state.resolver.resolve(&alias).await.unwrap();
    assert_eq!(
        restored
            .definition
            .secrets
            .telegram_webhook_secret
            .as_ref()
            .unwrap()
            .as_str(),
        WEBHOOK_SECRET
    );
    app.drain().await;
}

//! Compound creation/deletion resumes only completed or provably unsent steps.
use super::*;
use crate::{
    channels::{creation, management},
    providers::{ProviderClient, ProviderRegistry},
    replicas::mongo_fixture,
};

fn provider(app: &Native) -> Arc<ProviderClient> {
    let registry=ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":app.api_base,"kind":"responses_http","paths":{"agent_card":"/card","binding":"/provider/bindings/{binding_id}","conversation":"/provider/bindings/{binding_id}/{conversation_id}","responses":"/responses"},"max_inflight":8}]).to_string()).unwrap();
    Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 8).unwrap(),
    )
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_channel_create_resumes_after_terminal_webhook_refusal_without_rebinding() {
    let mongo = mongo_fixture::Fixture::new().await;
    let mut app = Native::with_fixture(Fixture::native(&mongo).await).await;
    let (definition, _) = app.reserve_child().await;
    app.fixture
        .forget_channel(definition.record_id.as_str())
        .await;
    app.service.management.records = Arc::new(
        (*app.service.management.records)
            .clone()
            .with_channel_mutation_custody(true),
    );
    app.service.management.providers = provider(&app);
    let mut request = app.replacement_request(&definition);
    request.record_id = Some(definition.record_id.clone());
    app.peer.rate_limit.store(true, Ordering::SeqCst);
    let creator = owner("alice", false);
    assert!(
        management::create(&app.service.management, &creator, request)
            .await
            .is_err()
    );
    let retained = creation::load_record(&app.service.management, &creator, &definition.record_id)
        .await
        .unwrap();
    let custody = retained.definition.mutation.as_ref().unwrap();
    assert!(custody.effect.is_none());
    assert_eq!(custody.completed.len(), 1);
    assert!(app.stored().await.0.webhook_lease.is_none());
    assert!(management::delete(
        &app.service.management,
        &creator,
        &retained.channel_id,
        retained.record.version
    )
    .await
    .is_err());
    let mut other = app.fixture.independent_service(&mongo).await;
    other.management.records = Arc::new(
        (*other.management.records)
            .clone()
            .with_channel_mutation_custody(true),
    );
    other.management.providers = provider(&app);
    other.management.telegram = app.service.management.telegram.clone();
    let mut request = app.replacement_request(&definition);
    request.record_id = Some(definition.record_id.clone());
    let created = management::create(&other.management, &creator, request)
        .await
        .unwrap();
    assert!(created.definition.creation.as_ref().unwrap().ready);
    assert!(
        creation::load_record(&other.management, &creator, &definition.record_id)
            .await
            .unwrap()
            .definition
            .mutation
            .is_none()
    );
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "provider_PUT")
            .count(),
        1
    );
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "setWebhook")
            .count(),
        2
    );
    // Disconnect succeeds; missing provider configuration proves zero unbind send.
    let saved = other.management.providers.clone();
    other.management.providers = Arc::new(
        ProviderClient::new(
            ProviderRegistry::parse("[]").unwrap(),
            Duration::from_secs(1),
            Duration::from_secs(1),
            8,
        )
        .unwrap(),
    );
    assert!(management::delete(
        &other.management,
        &creator,
        &created.channel_id,
        created.version
    )
    .await
    .is_err());
    let retained = creation::load_record(&other.management, &creator, &definition.record_id)
        .await
        .unwrap();
    assert!(retained
        .definition
        .mutation
        .as_ref()
        .unwrap()
        .completed
        .contains(&"disconnect_connection".into()));
    assert!(retained
        .definition
        .mutation
        .as_ref()
        .unwrap()
        .effect
        .is_none());
    other.management.providers = saved;
    management::delete(
        &other.management,
        &creator,
        &created.channel_id,
        created.version,
    )
    .await
    .unwrap();
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "provider_DELETE")
            .count(),
        1
    );
    assert_eq!(
        app.methods()
            .iter()
            .filter(|method| method.as_str() == "deleteWebhook")
            .count(),
        1
    );
    drop(other);
    drop(app);
    mongo.cleanup().await;
}

#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_channel_create_delete_retry_known_unsent_platform_reads() {
    let mongo = mongo_fixture::Fixture::new().await;
    let mut app = Native::with_fixture(Fixture::native(&mongo).await).await;
    let (definition, _) = app.reserve_child().await;
    app.fixture
        .forget_channel(definition.record_id.as_str())
        .await;
    app.service.management.records = Arc::new(
        (*app.service.management.records)
            .clone()
            .with_channel_mutation_custody(true),
    );
    app.service.management.providers = provider(&app);
    let mut request = app.replacement_request(&definition);
    request.record_id = Some(definition.record_id.clone());
    app.peer.fail_read.store(true, Ordering::SeqCst);
    let creator = owner("alice", false);
    assert!(
        management::create(&app.service.management, &creator, request)
            .await
            .is_err()
    );
    let retained = creation::load_record(&app.service.management, &creator, &definition.record_id)
        .await
        .unwrap();
    assert!(retained
        .definition
        .mutation
        .as_ref()
        .unwrap()
        .effect
        .is_none());
    let mut other = app.fixture.independent_service(&mongo).await;
    other.management.records = Arc::new(
        (*other.management.records)
            .clone()
            .with_channel_mutation_custody(true),
    );
    other.management.providers = provider(&app);
    other.management.telegram = app.service.management.telegram.clone();
    let mut request = app.replacement_request(&definition);
    request.record_id = Some(definition.record_id.clone());
    let created = management::create(&other.management, &creator, request)
        .await
        .unwrap();
    app.peer.fail_read.store(true, Ordering::SeqCst);
    assert!(management::delete(
        &other.management,
        &creator,
        &created.channel_id,
        created.version
    )
    .await
    .is_err());
    let retained = creation::load_record(&other.management, &creator, &definition.record_id)
        .await
        .unwrap();
    assert!(retained
        .definition
        .mutation
        .as_ref()
        .unwrap()
        .effect
        .is_none());
    management::delete(
        &app.service.management,
        &creator,
        &created.channel_id,
        created.version,
    )
    .await
    .unwrap();
    for method in [
        "provider_PUT",
        "provider_DELETE",
        "setWebhook",
        "deleteWebhook",
    ] {
        assert_eq!(
            app.methods()
                .iter()
                .filter(|value| value.as_str() == method)
                .count(),
            1,
            "{method}"
        );
    }
    drop(other);
    drop(app);
    mongo.cleanup().await;
}

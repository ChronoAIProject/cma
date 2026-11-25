use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn old_funding_recovery_keeps_committed_replacement_active_after_restart() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_funding_scope_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let fake = Harness::new();
    let start = layer2_core::seconds_from_rfc3339(&fake.sandbox.created_at).unwrap();
    fake.provider.now.store(start, Ordering::SeqCst);
    fake.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(fake.fund(start).await.is_err());
    let mut old = fake.saved().await;
    let pending = old.pending.clone().unwrap();
    old.revision = 0;
    MongoFundingStateStore(store.clone())
        .save(&mut old)
        .await
        .unwrap();

    let mut replacement = super::super::runtime_controls::replacement(&fake.sandbox);
    replacement.revision = 1;
    store.create_sandbox(replacement.clone()).await.unwrap();
    store
        .append_sandbox_event(
            &replacement.id,
            replacement.created_at.clone(),
            SandboxEventKind::Created,
        )
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let source = Arc::new(MongoFundingStateStore(reopened.clone()));
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    let control = Arc::new(super::super::super::mongo::RuntimeControl {
        store: reopened.clone(),
        adapter: Arc::new(adapter),
        deployment: "fixture".into(),
    });
    let funding = ProviderFundingAdapter::new(fake.provider.clone(), source.clone(), control);
    fake.provider.fail_recover.store(true, Ordering::SeqCst);
    assert_eq!(funding.renew_due(at(start + 10)).await.unwrap(), 0);
    let failed = source.load(&replacement.id).await.unwrap().unwrap();
    assert_eq!(failed.pending, Some(pending.clone()));
    assert!(failed.retry_at > start + 10);
    assert_eq!(
        reopened.get_sandbox(&replacement.id).await.unwrap(),
        Some(replacement.clone())
    );

    fake.provider.fail_recover.store(false, Ordering::SeqCst);
    assert_eq!(funding.renew_due(at(failed.retry_at)).await.unwrap(), 1);
    let recovered = source.load(&replacement.id).await.unwrap().unwrap();
    assert!(recovered.pending.is_none());
    assert_eq!(
        recovered.current.as_ref().unwrap().request_id,
        pending.request_id
    );
    assert_eq!(recovered.windows.len(), 1);
    assert_eq!(recovered.runtime_generation, 1);
    assert_eq!(fake.provider.requests.lock().unwrap().len(), 1);
    assert_eq!(
        reopened.get_sandbox(&replacement.id).await.unwrap(),
        Some(replacement)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_compute_stop_keeps_new_funding_row_and_due_visibility_after_restart() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_funding_stop_scope_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let mut fake = Harness::new();
    let old = fake.sandbox.clone();
    fake.sandbox = super::super::runtime_controls::replacement(&old);
    fake.fund(100).await.unwrap();
    let mut current = fake.saved().await;
    current.revision = 0;
    MongoFundingStateStore(store)
        .save(&mut current)
        .await
        .unwrap();
    let reopened = Arc::new(MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&client, &name).await.unwrap(),
    )));
    let funding = ProviderFundingAdapter::new(
        fake.provider.clone(),
        reopened.clone(),
        fake.control.clone(),
    );

    funding.settle(&old, at(110)).await.unwrap();

    assert_eq!(reopened.load(&old.id).await.unwrap(), Some(current.clone()));
    assert_eq!(reopened.due(110).await.unwrap(), vec![current]);
    funding.settle(&fake.sandbox, at(111)).await.unwrap();
    assert!(reopened.load(&old.id).await.unwrap().unwrap().stopped);
    client.database(&name).drop().await.unwrap();
}

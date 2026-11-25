use super::super::mongo::MongoFundingStateStore;
use super::*;
use layer2_core::sandbox::store::{SandboxEventStore as _, SandboxStore as _};
use layer2_core::sandbox::{SandboxEventKind, SandboxStatus};
use layer2_core::sandbox_usage_service::SandboxUsageStore as _;
mod runtime_controls;

mod missing_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn non_cohort_mongo_decision_survives_restart_and_is_not_due_for_funding() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_funding_cohort_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let source = Arc::new(MongoFundingStateStore(store));
    let fake = Harness::new();
    let sandbox = fake.sandbox.clone();
    let mut state = SandboxBillingState::new(&sandbox, 1);
    source.save(&mut state).await.unwrap();
    state.unbilled_generation = Some(1);
    source.save(&mut state).await.unwrap();
    assert!(source.due(500).await.unwrap().is_empty());
    let reopened = Arc::new(MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&client, &name).await.unwrap(),
    )));
    assert_eq!(reopened.load(&sandbox.id).await.unwrap(), Some(state));
    let adapter =
        ProviderFundingAdapter::new(fake.provider.clone(), reopened, fake.control.clone());
    assert_eq!(
        adapter.ensure_coverage(&sandbox, at(500)).await.unwrap(),
        None
    );
    adapter.settle(&sandbox, at(501)).await.unwrap();
    assert_eq!(adapter.renew_due(at(800)).await.unwrap(), 0);
    assert_eq!(fake.provider.quotes.load(Ordering::SeqCst), 0);
    assert!(fake.provider.requests.lock().unwrap().is_empty());
    assert!(fake.control.pauses.lock().unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn funding_mongo_cas_restart_retained_zero_evidence_and_due_recovery() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_funding_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let source = MongoFundingStateStore(store.clone());
    let mut sandbox = sandbox();
    sandbox.revision = 1;
    let start = layer2_core::seconds_from_rfc3339(&sandbox.created_at).unwrap();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store
        .append_sandbox_event(
            &sandbox.id,
            sandbox.created_at.clone(),
            SandboxEventKind::Created,
        )
        .await
        .unwrap();
    assert!(source.load(&sandbox.id).await.unwrap().is_none());
    assert_eq!(
        source.sandbox(&sandbox.id).await.unwrap().unwrap().id,
        sandbox.id
    );
    let mut state = SandboxBillingState::new(&sandbox, 1);
    let mut duplicate = state.clone();
    source.save(&mut state).await.unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(
        source.save(&mut duplicate).await,
        Err(FundingError::Conflict)
    );
    let mut stale = state.clone();
    state.retry_at = start + 20;
    source.save(&mut state).await.unwrap();
    assert_eq!(source.save(&mut stale).await, Err(FundingError::Conflict));
    assert!(source.due(start + 19).await.unwrap().is_empty());
    assert_eq!(source.due(start + 20).await.unwrap().len(), 1);
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let recovered = MongoFundingStateStore(reopened.clone());
    assert_eq!(recovered.load(&sandbox.id).await.unwrap(), Some(state));
    source
        .checkpoint(&sandbox.id, at(start + 60))
        .await
        .unwrap();
    let before = source.events(&sandbox.id, 0).await.unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(before[1].quantity.uptime_seconds, 60);
    sandbox.runtime = None;
    sandbox.lifecycle.home.as_mut().unwrap().released = true;
    sandbox.status = SandboxStatus::Retired;
    sandbox.retired_at = Some(layer2_core::rfc3339_at(at(start + 60)));
    sandbox.purge_after = Some(layer2_core::rfc3339_at(at(start + 3600)));
    sandbox.updated_at = layer2_core::rfc3339_at(at(start + 60));
    let revision = sandbox.revision;
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox.clone()).await.unwrap();
    let (a, b) = tokio::join!(
        source.checkpoint(&sandbox.id, at(start + 300)),
        recovered.checkpoint(&sandbox.id, at(start + 300))
    );
    a.unwrap();
    b.unwrap();
    let usage = reopened.usage(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(usage.last_observed_at, start + 300);
    assert_eq!(usage.totals.uptime_seconds, 60);
    let evidence = recovered.events(&sandbox.id, 2).await.unwrap();
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[1].allocation, Allocation::default());
    assert_eq!(
        evidence[1].quantity,
        layer2_core::sandbox_usage::UsageTotals::default()
    );
    let missing = SandboxId("sbx_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    assert!(source.sandbox(&missing).await.unwrap().is_none());
    source.checkpoint(&missing, at(start + 300)).await.unwrap();
    let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
    let adapter: Arc<dyn layer3_adapter::SandboxAdapter> = Arc::new(adapter);
    let control = super::super::mongo::RuntimeControl {
        store: store.clone(),
        adapter: adapter.clone(),
        deployment: "fixture".into(),
    };
    control
        .deadline(deadline(&sandbox, start + 400))
        .await
        .unwrap(); // no runtime needs no provider mutation
    control
        .pause(target(&sandbox), "unfunded", at(start + 300))
        .await
        .unwrap(); // retired is not eligible
    let mut active = fixture::sandbox();
    active.id = missing.clone();
    active.revision = 1;
    active.name = "funding-pause".into();
    active.lifecycle.home = None;
    store.create_sandbox(active.clone()).await.unwrap();
    assert!(matches!(
        control.deadline(deadline(&active, start + 400)).await,
        Err(FundingError::Retryable(_))
    ));
    control
        .pause(target(&active), "producer HTTP 402", at(start + 300))
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&active.id).await.unwrap().unwrap().status,
        SandboxStatus::Pausing
    );
    let disabled = production(None, store.clone(), adapter.clone(), "test".into()).unwrap();
    assert_eq!(
        disabled.ensure_coverage(&active, at(start)).await.unwrap(),
        None
    );
    production(
        Some(SandboxBillingConfig {
            base_url: "https://nyx.example/api/v1/billing/sandboxes/"
                .parse()
                .unwrap(),
            credential_ref: "NYX_TEST_SECRET_REFERENCE".into(),
            client_credentials: None,
            producer_id: uuid::Uuid::new_v4().to_string(),
        }),
        store,
        adapter,
        "test".into(),
    )
    .unwrap();
    client.database(&name).drop().await.unwrap();
}

fn target(sandbox: &Sandbox) -> FundingRuntime<'_> {
    FundingRuntime::for_state(
        &SandboxBillingState::new(sandbox, generation(sandbox)),
        sandbox,
    )
    .unwrap()
    .unwrap()
}

fn deadline(sandbox: &Sandbox, until: u64) -> FundedRuntime<'_> {
    target(sandbox)
        .confirmed(&FundingWindow {
            request_id: "fixture-request".into(),
            quote_id: "fixture-quote".into(),
            window_id: "fixture-window".into(),
            owner_id: sandbox.owner.to_string(),
            sandbox_id: sandbox.id.to_string(),
            runtime_generation: generation(sandbox),
            allocation: maximum_allocation(sandbox),
            covered_from: until - 300,
            covered_until: until,
            receipt: serde_json::json!({}),
        })
        .unwrap()
}

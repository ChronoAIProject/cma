use super::*;
use layer2_core::sandbox::store::SandboxWorkerStore as _;
use serde_json::json;

struct Fixture {
    client: mongodb::Client,
    name: String,
    store: Arc<MongoSandboxStore>,
    source: Arc<MongoFundingStateStore>,
    retained: SandboxBillingState,
    receipt: FundingWindow,
    now: u64,
}

impl Fixture {
    async fn purged() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let name = format!("cma_funding_missing_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let source = Arc::new(MongoFundingStateStore(store.clone()));
        let h = super::super::missing_tests::pending().await;
        let mut retired = h.sandbox.clone();
        let start = layer2_core::seconds_from_rfc3339(&retired.created_at).unwrap();
        retired.revision = 1;
        retired.status = SandboxStatus::Retired;
        retired.runtime = None;
        retired.lifecycle.home = None;
        retired.credential.current = None;
        retired.credential.delivered = false;
        retired.credential.key_generation = 0;
        retired.retired_at = Some(retired.created_at.clone());
        retired.purge_after = Some(retired.created_at.clone());
        retired.updated_at = retired.created_at.clone();
        retired.last_activity_at = retired.created_at.clone();
        store.create_sandbox(retired.clone()).await.unwrap();
        store
            .append_sandbox_event(
                &retired.id,
                retired.created_at.clone(),
                SandboxEventKind::Created,
            )
            .await
            .unwrap();
        source
            .checkpoint(&retired.id, at(start + 300))
            .await
            .unwrap();
        let mut retained = h.saved().await;
        retained.revision = 0;
        source.save(&mut retained).await.unwrap();
        let pending = retained.pending.as_ref().unwrap();
        let mut receipt = h.provider.receipts.lock().unwrap()[&pending.request_id].clone();
        receipt.covered_from = start;
        receipt.covered_until = start + 300;
        store
            .purge_sandbox(
                &retired.id,
                retired.revision,
                &layer2_core::rfc3339_at(at(start + 301)),
            )
            .await
            .unwrap();
        assert!(store.get_sandbox(&retired.id).await.unwrap().is_none());
        assert_eq!(
            source.load(&retired.id).await.unwrap(),
            Some(retained.clone())
        );
        Self {
            client,
            name,
            store,
            source,
            retained,
            receipt,
            now: start + 400,
        }
    }

    async fn close(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn purged_pending_receipt_is_discovered_after_restart_and_settles_only_real_retained_usage() {
    let f = Fixture::purged().await;
    let id = SandboxId(f.retained.sandbox_id.clone());
    assert_eq!(f.source.due(f.now).await.unwrap(), vec![f.retained.clone()]);
    let server = http::Server::new(vec![
        http::reply(200, serde_json::to_value(&f.receipt).unwrap()),
        http::reply(
            200,
            json!({
                "evidence_accepted":true, "window_id":f.receipt.window_id,
                "owner_id":f.receipt.owner_id, "sandbox_id":f.receipt.sandbox_id,
                "runtime_generation":f.receipt.runtime_generation
            }),
        ),
    ])
    .await;
    let control = Arc::new(Control::default());
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), f.source.clone(), control.clone());
    assert_eq!(adapter.renew_due(at(f.now)).await.unwrap(), 1);
    let saved = f.source.load(&id).await.unwrap().unwrap();
    assert!(saved.pending.is_none());
    assert_eq!(saved.windows.len(), 1);
    assert_eq!(
        saved.windows[0].window.request_id,
        f.retained.pending.as_ref().unwrap().request_id
    );
    assert_eq!(saved.operation_id, f.retained.operation_id);
    assert_eq!(server.state.calls.lock().unwrap().len(), 1);
    let reopened = Arc::new(MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&f.client, &f.name).await.unwrap(),
    )));
    let restarted =
        ProviderFundingAdapter::new(server.provider.clone(), reopened.clone(), control.clone());
    assert_eq!(restarted.renew_due(at(f.now + 61)).await.unwrap(), 1);
    let settled = reopened.load(&id).await.unwrap().unwrap();
    assert!(settled.pending.is_none() && settled.windows.is_empty());
    assert_eq!(
        settled.current.as_ref().unwrap().window_id,
        f.receipt.window_id
    );
    assert!(settled.evidence_cursor > 0);
    assert!(reopened.due(f.now + 122).await.unwrap().is_empty());
    assert!(f.store.get_sandbox(&id).await.unwrap().is_none());
    {
        let calls = server.state.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, axum::http::Method::GET);
        assert!(calls[0]
            .1
            .ends_with(&f.retained.pending.as_ref().unwrap().request_id));
        assert_eq!(calls[1].0, axum::http::Method::POST);
        assert!(calls[1]
            .1
            .ends_with(&format!("/windows/{}/evidence", f.receipt.window_id)));
        for interval in calls[1].3["intervals"].as_array().unwrap() {
            assert_eq!(interval["quantity"]["cpu_millis_seconds"], 0);
            assert_eq!(interval["quantity"]["memory_mib_seconds"], 0);
            assert_eq!(interval["quantity"]["disk_gib_seconds"], 0);
        }
    }
    assert!(control.deadlines.lock().unwrap().is_empty());
    assert!(control.pauses.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn competing_missing_sweeps_cas_before_receipt_lookup_and_retain_one_window() {
    let f = Fixture::purged().await;
    let server = http::Server::new(vec![http::reply(
        200,
        serde_json::to_value(&f.receipt).unwrap(),
    )])
    .await;
    let control = Arc::new(Control::default());
    let first =
        ProviderFundingAdapter::new(server.provider.clone(), f.source.clone(), control.clone());
    let reopened = Arc::new(MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&f.client, &f.name).await.unwrap(),
    )));
    let second =
        ProviderFundingAdapter::new(server.provider.clone(), reopened.clone(), control.clone());
    let (a, b) = tokio::join!(
        first.reconcile(f.retained.clone(), at(f.now)),
        second.reconcile(f.retained.clone(), at(f.now)),
    );
    assert!(matches!(
        (a, b),
        (Ok(()), Err(FundingError::Conflict)) | (Err(FundingError::Conflict), Ok(()))
    ));
    let saved = reopened
        .load(&SandboxId(f.retained.sandbox_id.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(saved.pending.is_none());
    assert_eq!(saved.windows.len(), 1);
    assert_eq!(server.state.calls.lock().unwrap().len(), 1);
    assert!(control.deadlines.lock().unwrap().is_empty());
    assert!(control.pauses.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn purged_unknown_receipt_remains_durable_due_without_new_reservation() {
    let f = Fixture::purged().await;
    let server = http::Server::new(vec![http::reply(404, json!({"error":"not_found"}))]).await;
    let control = Arc::new(Control::default());
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), f.source.clone(), control.clone());
    assert_eq!(adapter.renew_due(at(f.now)).await.unwrap(), 0);
    let reopened = MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&f.client, &f.name).await.unwrap(),
    ));
    let saved = reopened
        .load(&SandboxId(f.retained.sandbox_id.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.pending, f.retained.pending);
    assert_eq!(saved.current, f.retained.current);
    assert_eq!(saved.windows, f.retained.windows);
    assert!(reopened.due(f.now + 59).await.unwrap().is_empty());
    assert_eq!(reopened.due(f.now + 60).await.unwrap(), vec![saved]);
    assert_eq!(server.state.calls.lock().unwrap().len(), 1);
    assert!(control.deadlines.lock().unwrap().is_empty());
    assert!(control.pauses.lock().unwrap().is_empty());
    f.close().await;
}

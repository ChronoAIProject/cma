use super::*;

async fn retire(base: &str, record: &RecordId, version: u64, token: &str) -> reqwest::Response {
    Client::new()
        .delete(url(base, record))
        .query(&[("expected_version", version)])
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn legacy_retirement_fences_owner_and_version_and_accepts_missing_binding() {
    let mut fixture = Fixture::new().await;
    fixture.provider.close().await;
    fixture.provider = FakeProvider::start(FakeProviderConfig {
        unbind_status: [("synthetic-binding".into(), 404)].into(),
        ..FakeProviderConfig::default()
    })
    .await;
    let mut legacy = definition();
    legacy.secrets.provider_credential =
        Some(cmaeg_core::channel::Secret::new("fixture-provider-key"));
    fixture.seed(&legacy);
    let mut active = definition();
    active.creation = Some(creation(
        saved_id(&fixture.cma.base_url, &active.record_id, 1),
        true,
    ));
    fixture.seed(&active);
    let active_before = fixture.cma.records()[active.record_id.as_str()].clone();
    let mut state = fixture.state();
    state.records = Arc::new(
        cmaeg::records::RecordClient::new(&fixture.cma.base_url, "cma-service")
            .unwrap()
            .with_channel_mutation_custody(true),
    );
    let (base, stop, server) = serve_app(state).await;
    assert_eq!(
        read(get(&base, &legacy.record_id).await).await["record"]["retirement"],
        json!({"expected_version":1, "state":"available"})
    );
    for (record, version, token, status) in [
        (&legacy.record_id, 1, "token-bob", 404),
        (&legacy.record_id, 2, "token-alice", 409),
        (&active.record_id, 1, "token-alice", 400),
    ] {
        headers(&retire(&base, record, version, token).await, status);
    }
    let before = fixture.cma.records()[legacy.record_id.as_str()].clone();
    let refused = Client::new()
        .delete(url(&base, &legacy.record_id))
        .query(&[("expected_version", "1"), ("retire_unready", "true")])
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    headers(&refused, 400);
    let after = &fixture.cma.records()[legacy.record_id.as_str()];
    assert_eq!(after.version, before.version);
    assert_eq!(after.blob, before.blob);
    assert!(fixture.provider.requests().is_empty());
    headers(
        &retire(&base, &legacy.record_id, 1, "token-alice").await,
        204,
    );
    headers(
        &retire(&base, &legacy.record_id, 1, "token-alice").await,
        404,
    );
    headers(&get(&base, &legacy.record_id).await, 404);
    assert_eq!(fixture.provider.requests().len(), 1);
    let records = fixture.cma.records();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[active.record_id.as_str()].version,
        active_before.version
    );
    assert_eq!(records[active.record_id.as_str()].blob, active_before.blob);
    stop.send(()).unwrap();
    server.await.unwrap();
    fixture.provider.close().await;
    fixture.cma.close().await;
    fixture.nyxid.close().await;
}

#[tokio::test]
async fn legacy_provider_failure_retains_owner_visible_cleanup_custody() {
    let mut fixture = Fixture::new().await;
    fixture.provider.close().await;
    fixture.provider = FakeProvider::start(FakeProviderConfig {
        unbind_status: [("synthetic-binding".into(), 503)].into(),
        ..FakeProviderConfig::default()
    })
    .await;
    let mut legacy = definition();
    legacy.secrets.provider_credential =
        Some(cmaeg_core::channel::Secret::new("fixture-provider-key"));
    fixture.seed(&legacy);
    let mut state = fixture.state();
    state.records = Arc::new(
        cmaeg::records::RecordClient::new(&fixture.cma.base_url, "cma-service")
            .unwrap()
            .with_channel_mutation_custody(true),
    );
    let (base, stop, server) = serve_app(state).await;
    let failure = retire(&base, &legacy.record_id, 1, "token-alice").await;
    assert!(failure.status().is_server_error());
    let readback = read(get(&base, &legacy.record_id).await).await;
    assert_eq!(readback["record"]["pending_operation"], "delete");
    assert_eq!(
        readback["record"]["retirement"],
        json!({"expected_version":1, "state":"cleanup_unconfirmed"})
    );
    headers(
        &retire(&base, &legacy.record_id, 1, "token-alice").await,
        409,
    );
    assert_eq!(fixture.provider.requests().len(), 1);
    stop.send(()).unwrap();
    server.await.unwrap();
    fixture.provider.close().await;
    fixture.cma.close().await;
    fixture.nyxid.close().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn mongo_legacy_retirement_cleans_a_pre_creation_record() {
    use crate::binary_support::{assert_no_secrets, free_address, spawn_ready, MountedSecrets};
    use mongodb::bson::{doc, spec::BinarySubtype, Binary, Document};

    let mut fixture = Fixture::new().await;
    fixture.provider.close().await;
    fixture.provider = FakeProvider::start(FakeProviderConfig {
        unbind_status: [("synthetic-binding".into(), 404)].into(),
        ..FakeProviderConfig::default()
    })
    .await;
    let files = MountedSecrets::new();
    let address = free_address();
    let base = format!("http://{address}");
    let log = files.dir.join("legacy-retirement.log");
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let mut record = definition();
    record.secrets.provider_credential =
        Some(cmaeg_core::channel::Secret::new("fixture-provider-key"));
    let db = database.admin.database(&database.name);
    let collection = db.collection::<Document>("eg_channel_records");
    let key = doc! {"_id":record.record_id.as_str()};
    collection.insert_one(doc! {
        "_id":record.record_id.as_str(), "owner":"alice", "version":1_i64, "schema_version":1_i32,
        "blob":Binary { subtype:BinarySubtype::Generic, bytes:blob(&record) },
    }).await.unwrap();
    let mut command = files.command(&address, &fixture.cma, &fixture.nyxid, &fixture.provider);
    database.configure(&mut command);
    command.env("CMAEG_CHANNEL_MUTATION_CUSTODY_ENABLED", "true");
    let mut child = spawn_ready(command, &address, &log).await;
    fixture.cma.close().await;
    headers(&retire(&base, &record.record_id, 1, "token-bob").await, 404);
    headers(
        &retire(&base, &record.record_id, 1, "token-alice-manager").await,
        403,
    );
    headers(
        &retire(&base, &record.record_id, 2, "token-alice").await,
        409,
    );
    assert!(collection.find_one(key.clone()).await.unwrap().is_some());
    assert!(fixture.provider.requests().is_empty());
    headers(
        &retire(&base, &record.record_id, 1, "token-alice").await,
        204,
    );
    headers(
        &retire(&base, &record.record_id, 1, "token-alice").await,
        404,
    );
    assert!(collection.find_one(key).await.unwrap().is_none());
    assert_eq!(fixture.provider.requests().len(), 1);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert_no_secrets(&std::fs::read_to_string(log).unwrap());
    db.drop().await.unwrap();
    fixture.provider.close().await;
    fixture.nyxid.close().await;
}

#[tokio::test]
async fn missing_provider_credential_is_blocked_without_reserving_deletion() {
    let fixture = Fixture::new().await;
    let mut legacy = definition();
    legacy.secrets.provider_credential = None;
    fixture.seed(&legacy);
    let before = fixture.cma.records();
    let mut state = fixture.state();
    state.records = Arc::new(
        cmaeg::records::RecordClient::new(&fixture.cma.base_url, "cma-service")
            .unwrap()
            .with_channel_mutation_custody(true),
    );
    let (base, stop, server) = serve_app(state).await;
    for _ in 0..3 {
        let record = read(get(&base, &legacy.record_id).await).await;
        assert_eq!(
            record["record"]["retirement"],
            json!({
                "expected_version":1,
                "state":"blocked_missing_credential"
            })
        );
        assert!(record["record"]["pending_operation"].is_null());
        let refusal = retire(&base, &legacy.record_id, 1, "token-alice").await;
        headers(&refusal, 400);
        let problem: Value = refusal.json().await.unwrap();
        assert!(problem.to_string().contains("original channel credential"));
    }
    let after = fixture.cma.records();
    assert_eq!(after[legacy.record_id.as_str()].version, 1);
    assert_eq!(
        after[legacy.record_id.as_str()].blob,
        before[legacy.record_id.as_str()].blob
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    fixture.close().await;
}

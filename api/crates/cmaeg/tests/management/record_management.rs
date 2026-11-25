//! Inspection preserves retained identity even without a recoverable address.
use super::*;
use cmaeg_core::{
    channel::{ChannelCreation, ChannelDefinition, ChannelMutation, ChannelMutationKind},
    ids::{ChannelId, RecordId},
};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "record_management/address.rs"]
mod address;
#[path = "record_management/boundaries.rs"]
mod boundaries;
#[path = "record_management/retirement.rs"]
mod retirement;
#[path = "record_management/support.rs"]
mod support;
use support::*;

#[tokio::test]
async fn legacy_read_and_head_preserve_inventory_without_provider_or_index_effects() {
    let fixture = Fixture::new().await;
    let legacy = definition();
    fixture.seed(&legacy);
    let mut active = definition();
    active.creation = Some(creation(
        saved_id(&fixture.cma.base_url, &active.record_id, 1),
        true,
    ));
    fixture.seed(&active);
    let before = fixture.cma.records();
    let (base, stop, task) = serve_app(fixture.state()).await;
    for _ in 0..3 {
        assert_eq!(
            read(get(&base, &legacy.record_id).await).await,
            json!({
                "record_id":legacy.record_id, "version":1, "record":{
                    "state":"valid", "name":"support-bot", "owner":{"kind":"human","subject":"alice"},
                    "provider":{"slug":"cma_codex","level":"l1"}, "creation":"legacy", "address":{"state":"not_recorded"}
                }
            })
        );
        let response = Client::new()
            .head(url(&base, &legacy.record_id))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        headers(&response, 200);
        assert!(response.bytes().await.unwrap().is_empty());
        assert_eq!(
            read(get(&base, &active.record_id).await).await["record"]["address"]["state"],
            "available"
        );
    }
    // The compatibility recovery route still refuses a legacy record with no saved address.
    let response = Client::new()
        .get(format!("{base}/v1/channel-records/{}", legacy.record_id))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    headers(&response, 404);
    let after = fixture.cma.records();
    assert_eq!(before.len(), after.len());
    for (id, row) in before {
        assert_eq!(after[&id].blob, row.blob);
        assert_eq!(after[&id].version, row.version);
        assert_eq!(after[&id].owner, row.owner);
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn owned_invalid_definition_returns_only_record_identity_and_storage_version() {
    let fixture = Fixture::new().await;
    let mut invalid = definition();
    invalid.name = "private-invalid-name".into();
    invalid.sources.clear();
    invalid.creation = Some(creation(
        saved_id(&fixture.cma.base_url, &invalid.record_id, 1),
        true,
    ));
    assert!(invalid.validate().is_err());
    fixture.seed(&invalid);
    let (base, stop, task) = serve_app(fixture.state()).await;
    assert_eq!(
        read(get(&base, &invalid.record_id).await).await,
        json!({
            "record_id":invalid.record_id, "version":1, "record":{"state":"invalid"}
        })
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn pending_operation_is_bounded_and_version_is_the_storage_version() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    for (operation, label) in [
        (ChannelMutationKind::Create, "create"),
        (ChannelMutationKind::Update, "update"),
        (ChannelMutationKind::Delete, "delete"),
        (ChannelMutationKind::AuditRouting, "audit_routing"),
        (ChannelMutationKind::AuditAttachments, "audit_attachments"),
    ] {
        let mut record = definition();
        let nonce = uuid::Uuid::new_v4().to_string();
        let operation_id = uuid::Uuid::new_v4().to_string();
        record.mutation = Some(ChannelMutation {
            nonce: nonce.clone(),
            operation_id: operation_id.clone(),
            operation,
            original_version: 7,
            fingerprint: "private-fingerprint".into(),
            intended: "private-intended".into(),
            completed: vec!["private-completed".into()],
            effect: Some("private-effect".into()),
        });
        record.validate().unwrap();
        fixture.seed(&record);
        let result = read(get(&base, &record.record_id).await).await;
        assert_eq!(result["version"], 1);
        assert_eq!(result["record"]["pending_operation"], label);
        for hidden in [
            &nonce,
            &operation_id,
            "original_version",
            "binding",
            "profile",
            "fingerprint",
            "intended",
            "completed",
            "effect",
        ] {
            assert!(!result.to_string().contains(hidden));
        }
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn mongo_legacy_read_is_owned_and_read_only() {
    use super::binary_support::{assert_no_secrets, free_address, spawn_ready, MountedSecrets};
    use mongodb::bson::{doc, spec::BinarySubtype, Binary, Document};
    let fixture = Fixture::new().await;
    let files = MountedSecrets::new();
    let address = free_address();
    let base = format!("http://{address}");
    let log = files.dir.join("trace.log");
    let database =
        database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
    let record = definition();
    let row = doc! {"_id":record.record_id.as_str(), "owner":"alice", "version":1_i64, "schema_version":1_i32,
    "blob":Binary { subtype:BinarySubtype::Generic, bytes:blob(&record) }};
    let collection = database
        .admin
        .database(&database.name)
        .collection::<Document>("eg_channel_records");
    collection.insert_one(row.clone()).await.unwrap();
    let mut command = files.command(&address, &fixture.cma, &fixture.nyxid, &fixture.provider);
    database.configure(&mut command);
    let mut child = spawn_ready(command, &address, &log).await;
    // A successful read must use the independent Gateway store.
    fixture.cma.close().await;
    for _ in 0..2 {
        let response = Client::new()
            .get(url(&base, &record.record_id))
            .bearer_auth("token-alice-manager")
            .send()
            .await
            .unwrap();
        let body = read(response).await;
        assert_eq!(body["record"]["creation"], "legacy");
        assert_eq!(body["record"]["address"], json!({"state":"not_recorded"}));
        assert_eq!(body["version"], 1);
    }
    for (token, status) in [
        (Some("token-bob"), 404),
        (Some("token-alice-create-only"), 403),
        (None, 401),
        (Some("invalid-token"), 401),
    ] {
        let mut request = Client::new().get(url(&base, &record.record_id));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        headers(&request.send().await.unwrap(), status);
    }
    let response = Client::new()
        .head(url(&base, &record.record_id))
        .bearer_auth("token-alice-manager")
        .send()
        .await
        .unwrap();
    headers(&response, 200);
    assert!(response.bytes().await.unwrap().is_empty());
    assert_eq!(
        collection
            .find_one(doc! {"_id":record.record_id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        row
    );
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
    assert!(fixture.provider.requests().is_empty());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert_no_secrets(&std::fs::read_to_string(log).unwrap());
    database
        .admin
        .database(&database.name)
        .drop()
        .await
        .unwrap();
    fixture.provider.close().await;
    fixture.nyxid.close().await;
}

#[tokio::test]
async fn management_read_does_not_probe_unavailable_or_removed_providers() {
    let fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let probe = axum::Router::new().fallback(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { axum::http::StatusCode::SERVICE_UNAVAILABLE }
    });
    let (provider_url, provider_stop, provider_task) = super::recovery::serve_fixture(probe).await;
    let mut state = fixture.state();
    state.providers = Arc::new(
        ProviderClient::new(
            registry(&provider_url),
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(100),
            8,
        )
        .unwrap(),
    );
    let (base, stop, task) = serve_app(state).await;
    for slug in ["cma_codex", "retired_provider"] {
        let mut input = create_body();
        input["profile"]["metadata"]["provider"] = slug.into();
        input["provider"]["slug"] = slug.into();
        let record = definition_from(input);
        record.validate().unwrap();
        fixture.seed(&record);
        let result = read(get(&base, &record.record_id).await).await;
        assert_eq!(result["record"]["provider"]["slug"], slug);
        assert_eq!(result["record"]["state"], "valid");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    stop.send(()).unwrap();
    task.await.unwrap();
    provider_stop.send(()).unwrap();
    provider_task.await.unwrap();
    fixture.close().await;
}

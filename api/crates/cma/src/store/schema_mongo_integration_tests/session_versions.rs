//! Regression for the desktop lease writer shipped in #1341 / b31a88603.

use std::sync::{Arc, Mutex};

use futures_util::TryStreamExt;
use tracing::instrument::WithSubscriber;

use layer2_core::sandbox::store::{SandboxSessionStore, SandboxStore, SandboxStoreError};
use layer2_core::sandbox::RuntimeBinding;
use layer2_core::{SandboxId, StoreError};
use layer3_adapter::hosted::RuntimeRoutingStore;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{Client, Collection};

use super::database;
use crate::store::runtime_routing::MongoRuntimeRouting;
use crate::store::sandbox_mongo::{
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, SANDBOXES, SANDBOX_SESSIONS,
};
use crate::store::sandbox_mongo_integration_tests::sandbox;
use crate::store::sandboxes::sandbox_document;
use crate::store::schema::refuse_unknown_versions;

#[derive(Clone, Default)]
struct TraceWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TraceWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn old_writer_document() -> Document {
    // Exact insert document from sandbox_sessions.rs:24-28 at b31a88603.
    doc! {
        "_id": "desktop-lease",
        "sandbox_id": "sbx_session_version",
        "lease_expires_at": "2026-11-06T00:01:30Z",
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn session_writer_persists_version_and_restart_succeeds() {
    let (client, name) = database().await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_session_version");
    store.create_sandbox(parent.clone()).await.unwrap();
    let rows = db.collection::<Document>(SANDBOX_SESSIONS);
    store
        .open_sandbox_session(&parent.id, "desktop-lease", "2026-11-06T00:01:30Z")
        .await
        .unwrap();
    let mut expected = old_writer_document();
    expected.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    assert_eq!(rows.find_one(doc! {}).await.unwrap(), Some(expected));
    MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(matches!(
        store
            .open_sandbox_session(&parent.id, "desktop-lease", "2026-11-06T00:01:30Z")
            .await,
        Err(SandboxStoreError::Conflict)
    ));

    // A retained lease is stamped on renewal without changing its identity.
    rows.update_one(doc! {}, doc! {"$unset": {"document_schema_version": ""}})
        .await
        .unwrap();
    store
        .renew_sandbox_session(&parent.id, "desktop-lease", "2026-11-06T00:03:00Z")
        .await
        .unwrap();
    let renewed = rows.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(renewed.get_i64("document_schema_version").unwrap(), 1);
    assert_eq!(
        renewed.get_str("lease_expires_at").unwrap(),
        "2026-11-06T00:03:00Z"
    );
    for version in [
        Bson::Int64(2),
        Bson::Null,
        Bson::Double(1.0),
        Bson::Array(vec![1_i64.into()]),
    ] {
        rows.update_one(
            doc! {},
            doc! {"$set": {"document_schema_version": &version}},
        )
        .await
        .unwrap();
        assert!(matches!(
            store
                .renew_sandbox_session(&parent.id, "desktop-lease", "2026-11-06T00:04:00Z")
                .await,
            Err(SandboxStoreError::NotFound)
        ));
        let unchanged = rows.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(unchanged.get("document_schema_version"), Some(&version));
        assert_eq!(
            unchanged.get_str("lease_expires_at").unwrap(),
            "2026-11-06T00:03:00Z"
        );
    }
    assert!(store
        .open_sandbox_session(&parent.id, "", "invalid")
        .await
        .is_err());
    assert!(store
        .open_sandbox_session(
            &SandboxId("sbx_missing".into()),
            "missing",
            "2026-11-06T00:01:30Z"
        )
        .await
        .is_err());
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn old_writer_reproduces_old_startup_refusal_but_known_v1_lease_reopens() {
    let (client, name) = database().await;
    let db = client.database(&name);
    crate::store::migrations::prepare_bindings_for_test(&db).await;
    let rows = db.collection::<Document>(SANDBOX_SESSIONS);
    let legacy = old_writer_document();
    rows.insert_one(legacy.clone()).await.unwrap();
    let mut expired = legacy.clone();
    expired.insert("_id", "expired-desktop-lease");
    expired.insert("lease_expires_at", "2026-11-05T00:01:30Z");
    rows.insert_one(expired.clone()).await.unwrap();
    // The exact deployed startup predicate finds the old writer's own row.
    let old_rejection = doc! {"$or": [
        {"document_schema_version": {"$ne": 1_i64}},
        {"document_schema_version": {"$not": {"$type": ["int", "long"]}}},
        {"document_schema_version": {"$type": "array"}},
    ]};
    assert!(rows.find_one(old_rejection).await.unwrap().is_some());
    let output = TraceWriter::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let store = MongoSandboxStore::open(&client, &name)
        .with_subscriber(subscriber)
        .await
        .unwrap();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 1);
    assert!(logs.contains("WARN") && logs.contains("count=2"));
    for field in ["_id", "sandbox_id", "lease_expires_at"] {
        assert!(!logs.contains(legacy.get_str(field).unwrap()));
    }
    assert!(store
        .has_open_sandbox_session(
            &SandboxId("sbx_session_version".into()),
            "2026-11-06T00:00:30Z"
        )
        .await
        .unwrap());
    assert!(!store
        .has_open_sandbox_session(
            &SandboxId("sbx_session_version".into()),
            "2026-11-06T00:02:00Z"
        )
        .await
        .unwrap());
    // Startup is read-only: no backfill or new reader requirements.
    assert_eq!(
        rows.find_one(doc! {"_id": "desktop-lease"}).await.unwrap(),
        Some(legacy)
    );
    assert_eq!(
        rows.find_one(doc! {"_id": "expired-desktop-lease"})
            .await
            .unwrap(),
        Some(expired)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn startup_rejects_explicit_unknown_versions_and_unrecognized_legacy_shapes() {
    let (client, name) = database().await;
    let db = client.database(&name);
    let rows = db.collection::<Document>(SANDBOX_SESSIONS);
    for version in [Bson::Int32(1), Bson::Int64(1)] {
        let mut row = old_writer_document();
        row.insert("document_schema_version", version);
        rows.insert_one(row).await.unwrap();
        refuse_unknown_versions(&db, &[SANDBOX_SESSIONS])
            .await
            .unwrap();
        rows.delete_many(doc! {}).await.unwrap();
    }
    let mut invalid = Vec::new();
    for version in [
        Bson::Int64(0),
        Bson::Int64(2),
        Bson::Int64(99),
        Bson::String("1".into()),
        Bson::Null,
        Bson::Double(1.0),
        Bson::Array(vec![1_i64.into()]),
    ] {
        let mut row = old_writer_document();
        row.insert("document_schema_version", version);
        invalid.push(row);
    }
    for field in ["_id", "sandbox_id", "lease_expires_at"] {
        for value in [
            Bson::Null,
            Bson::Int32(1),
            Bson::String(String::new()),
            Bson::Array(vec!["value".into()]),
        ] {
            if field == "_id" && matches!(value, Bson::Array(_)) {
                continue; // Mongo itself prohibits array-valued primary keys.
            }
            let mut row = old_writer_document();
            row.insert(field, value);
            invalid.push(row);
        }
    }
    let mut extra = old_writer_document();
    extra.insert("future_field", true);
    invalid.push(extra);
    let mut missing = old_writer_document();
    missing.remove("sandbox_id");
    invalid.push(missing);
    let mut bad_expiry = old_writer_document();
    bad_expiry.insert("lease_expires_at", "not-a-timestamp");
    invalid.push(bad_expiry);
    for row in invalid {
        // A valid legacy row must not mask another unsupported document.
        rows.insert_one(old_writer_document()).await.unwrap();
        let mut row = row;
        if row.get_str("_id").ok() == Some("desktop-lease") {
            row.insert("_id", "invalid-lease");
        }
        rows.insert_one(row).await.unwrap();
        assert!(matches!(
            refuse_unknown_versions(&db, &[SANDBOX_SESSIONS]).await,
            Err(StoreError::Malformed(_))
        ));
        rows.delete_many(doc! {}).await.unwrap();
    }
    db.collection::<Document>("unrecognized_collection")
        .insert_one(old_writer_document())
        .await
        .unwrap();
    assert!(matches!(
        refuse_unknown_versions(&db, &["unrecognized_collection"]).await,
        Err(StoreError::Malformed(_))
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn routing_upserts_persist_version_on_insert_and_keep_existing_rows() {
    let (client, name) = database().await;
    let db = client.database(&name);
    let routing = MongoRuntimeRouting::open(&db, "schema-test", true)
        .await
        .unwrap();
    assert!(routing.select("sbx_route", 1, true).await.unwrap());
    assert!(routing.select("sbx_route", 1, false).await.unwrap());
    let rows = db.collection::<Document>("runtime_backend_routes");
    // Both startup marker and provider selection use $setOnInsert upserts.
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        rows.count_documents(doc! {"document_schema_version": DOCUMENT_SCHEMA_VERSION})
            .await
            .unwrap(),
        2
    );
    MongoRuntimeRouting::open(&db, "schema-test", true)
        .await
        .unwrap();
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 2);
    db.drop().await.unwrap();
}

async fn routing_choices(rows: &Collection<Document>) -> Vec<Document> {
    rows.find(doc! {"sandbox_id":{"$exists":true}})
        .sort(doc! {"sandbox_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn routing_migration_resumes_after_initialization_marker_write_fails() {
    const DEPLOYMENT: &str = "routing-recovery";
    const ROUTES: &str = "runtime_backend_routes";
    let (client, name) = database().await;
    let db = client.database(&name);
    let marker_id = format!("provider-routing-initialized:{DEPLOYMENT}");

    let mut bound = sandbox("sbx_route_bound");
    bound.lifecycle.runtime_generation_high_water = 5;
    bound.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: "agent-sandbox:fixture:runtime".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(5),
        bound_at: bound.created_at.clone(),
    });
    let mut unbound = sandbox("sbx_route_unbound");
    unbound.lifecycle.runtime_generation_high_water = 3;
    db.collection::<Document>(SANDBOXES)
        .insert_many([
            sandbox_document(&bound, 0).unwrap(),
            sandbox_document(&unbound, 0).unwrap(),
        ])
        .await
        .unwrap();

    // Let every route write commit while rejecting only this deployment's
    // final marker in the disposable database.
    db.create_collection(ROUTES)
        .validator(doc! {"_id":{"$ne":&marker_id}})
        .await
        .unwrap();
    assert_eq!(
        MongoRuntimeRouting::open(&db, DEPLOYMENT, false)
            .await
            .err()
            .as_deref(),
        Some("cannot record provider routing migration")
    );

    let marker = doc! {"_id":&marker_id};
    let rows = db.collection::<Document>(ROUTES);
    assert_eq!(rows.find_one(marker.clone()).await.unwrap(), None);
    let retained = routing_choices(&rows).await;
    let expected = [
        (bound.id.as_str(), 5_u64, true, "agent-sandbox"),
        (unbound.id.as_str(), 3, false, "opensandbox"),
    ];
    assert_eq!(retained.len(), expected.len());
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 2);
    for (row, (sandbox_id, generation, _, backend)) in retained.iter().zip(expected) {
        let id =
            layer3_adapter::provider_request_digest(&(DEPLOYMENT, sandbox_id, generation)).unwrap();
        assert_eq!(
            row,
            &doc! {
                "_id":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "sandbox_id":sandbox_id,"generation":Bson::Int64(generation.try_into().unwrap()),
                "deployment":DEPLOYMENT,"backend":backend
            }
        );
    }

    db.run_command(doc! {"collMod":ROUTES,"validator":{}})
        .await
        .unwrap();
    let restarted = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = restarted.database(&name);
    let routing = MongoRuntimeRouting::open(&db, DEPLOYMENT, true)
        .await
        .unwrap();
    let rows = db.collection::<Document>(ROUTES);
    assert_eq!(routing_choices(&rows).await, retained);
    for (sandbox_id, generation, selected, _) in expected {
        assert_eq!(
            routing
                .select(sandbox_id, generation, !selected)
                .await
                .unwrap(),
            selected
        );
    }
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(rows.count_documents(marker.clone()).await.unwrap(), 1);
    assert_eq!(
        rows.find_one(marker).await.unwrap(),
        Some(doc! {
            "_id":marker_id,"deployment":DEPLOYMENT,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION
        })
    );
    assert_eq!(routing_choices(&rows).await, retained);
    db.drop().await.unwrap();
}

use super::*;
use crate::document_artifacts::MongoDocumentStore;
use layer2_core::{Authorization, Caller, Capability, SubjectId};

#[test]
fn document_quota_inventory_requires_complete_bounded_exact_unique_sources() {
    let id = uuid::Uuid::new_v4().to_string();
    let source = source("owner", 1, b"source");
    let mut value = serde_json::json!({"schema_version": 1, "activation_id": id, "complete_storage_inventory": true, "writers_drained": true, "objects": [source]});
    let parse = |value: &serde_json::Value| Inventory::parse(&serde_json::to_vec(value).unwrap());
    assert!(parse(&value).is_ok());
    for field in ["complete_storage_inventory", "writers_drained"] {
        value[field] = false.into();
        assert!(parse(&value).is_err());
        value[field] = true.into();
    }
    value["objects"] = serde_json::json!([source, source]);
    assert!(parse(&value).is_err());
    value["objects"] = serde_json::json!([source]);
    value["objects"][0]["bytes"] = 0.into();
    assert!(parse(&value).is_err());
    value["objects"] = serde_json::json!([source]);
    value["schema_version"] = 2.into();
    assert!(parse(&value).is_err());
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_baseline_checks_retained_references_before_any_install_and_rejects_wrong_bytes(
) {
    let database = database().await;
    let metadata = Arc::new(MongoDocumentStore::new(database.clone()));
    metadata.ensure_indexes().await.unwrap();
    let service = DocumentService::new(
        metadata.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let caller = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let created = service
        .create(
            &caller,
            "legacy",
            CreateDocument {
                title: "Legacy".into(),
                markdown: "retained".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let quota = MongoDocumentQuotaStore::new(&database, true);
    let id = uuid::Uuid::new_v4().to_string();
    quota.prepare(&id).await.unwrap();
    assert_eq!(
        quota.install(&database, &inventory(&id, &[]), true).await,
        Err(DocumentError::Corrupt)
    );
    assert_eq!(quota.rows.count_documents(doc! {}).await.unwrap(), 1);
    let source = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: created.id,
        sha256: created.sha256,
        bytes: created.size_bytes,
    };
    let wrong = DocumentSourceReservation {
        bytes: source.bytes + 1,
        ..source.clone()
    };
    assert_eq!(
        quota
            .install(&database, &inventory(&id, &[wrong]), true)
            .await,
        Err(DocumentError::Corrupt)
    );
    let inventory = inventory(&id, std::slice::from_ref(&source));
    quota.install(&database, &inventory, false).await.unwrap();
    assert_eq!(quota.rows.count_documents(doc! {}).await.unwrap(), 1); // dry-run does not bind or install
    assert_eq!(
        quota.reserve(&source).await,
        Err(DocumentError::Unavailable)
    );
    quota.install(&database, &inventory, true).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 8, 1]);
    assert!(service
        .source(&caller, &source.document_id, None, None)
        .await
        .is_ok());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_partial_install_resumes_exact_manifest_and_never_overwrites_a_corrupt_row()
{
    let database = database().await;
    let quota = MongoDocumentQuotaStore::new(&database, true);
    let id = uuid::Uuid::new_v4().to_string();
    quota.prepare(&id).await.unwrap();
    let source = source("owner", 1, b"body");
    let inventory = inventory(&id, std::slice::from_ref(&source));
    database.run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {"kind": {"$ne": "object"}}, "validationLevel": "strict"}).await.unwrap();
    assert_eq!(
        quota.install(&database, &inventory, true).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(quota.rows.count_documents(doc! {}).await.unwrap(), 3); // state, owner, document
    assert_eq!(
        quota.reserve(&source).await,
        Err(DocumentError::Unavailable)
    );
    let changed = self::inventory(&id, &[]);
    assert_eq!(
        quota.install(&database, &changed, true).await,
        Err(DocumentError::Conflict)
    );
    database
        .run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {}})
        .await
        .unwrap();
    quota.install(&database, &inventory, true).await.unwrap();
    quota.reserve(&source).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    let object = object_row(&source).unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id": object.get_str("_id").unwrap()},
            doc! {"$set": {"bytes": 99_i64}},
        )
        .await
        .unwrap();
    assert_eq!(quota.reserve(&source).await, Err(DocumentError::Corrupt));
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    database.drop().await.unwrap();
}

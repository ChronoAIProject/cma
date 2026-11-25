use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_worker_reaches_the_fifth_accounting_entry() {
    verify_worker_accounting_page(true).await;
    verify_worker_accounting_page(false).await;
}

async fn verify_worker_accounting_page(generation_writes: bool) {
    use crate::config::DocumentStorageConfig;
    use crate::dependency_test_support::Peer;
    use crate::document_artifacts::ReclamationWorker;
    use crate::document_storage::maintenance::StorageMaintenance;
    use std::time::Duration;

    let database = database().await;
    let documents = MongoDocumentStore::new(database.clone())
        .with_source_commits(true)
        .with_generation_writes(generation_writes);
    documents.ensure_indexes().await.unwrap();
    let quota = documents.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    let service = DocumentService::with_storage_quota(
        Arc::new(documents.clone()),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        false,
        Some(Arc::new(documents.quota(true))),
    )
    .with_deletion_enabled(true);
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut created = Vec::new();
    for key in ["first", "second", "third", "fourth", "fifth"] {
        created.push(
            service
                .create(
                    &owner,
                    key,
                    CreateDocument {
                        title: "Document".into(),
                        markdown: "body".into(),
                        origin_agent_id: None,
                    },
                )
                .await
                .unwrap(),
        );
    }
    if !generation_writes {
        // An unmarked predecessor source can still have a live reference.
        let upgraded = documents.clone().with_generation_writes(true).quota(true);
        for document in &created {
            upgraded
                .reserve(&DocumentSourceReservation {
                    owner_hash: digest(b"owner"),
                    document_id: document.id.clone(),
                    sha256: document.sha256.clone(),
                    bytes: document.size_bytes,
                })
                .await
                .unwrap();
        }
    }
    created.sort_by(|a, b| a.id.cmp(&b.id));
    let deleted = created.pop().unwrap();
    service
        .delete(&owner, &deleted.id, "delete", 1)
        .await
        .unwrap();
    quota
        .rows
        .update_many(
            doc! { "kind": "source_commit" },
            doc! { "$set": { "updated_at": DateTime::from_millis(1) } },
        )
        .await
        .unwrap();

    let mut peer = Peer::start().await;
    let storage = StorageMaintenance::new(
        DocumentStorageConfig {
            resilience_enabled: true,
            platform_origin: peer.origin.clone(),
            bucket: "fixture".into(),
            client_id: "fixture-machine".into(),
            client_secret: "local-fixture".to_owned().into(),
        },
        true,
    )
    .unwrap();
    quota
        .bind_storage(
            &baseline.activation,
            &baseline.digest,
            &storage.namespace_sha256(),
        )
        .await
        .unwrap();
    let mut worker = ReclamationWorker::prepare(documents.clone(), storage)
        .await
        .unwrap();
    worker.run_once().await.unwrap();
    assert_eq!(peer.count(), 0);
    assert_eq!(usage(&quota, "owner").await, [5, 20, 5]);
    let expired = if generation_writes {
        Vec::new()
    } else {
        seed_expired_claims(&quota).await
    };

    let key = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: deleted.id,
        sha256: deleted.sha256,
        bytes: deleted.size_bytes,
    }
    .object_key();
    let responses = async {
        peer.next()
            .await
            .json(serde_json::json!({"access_token":"fixture", "expires_in":3600}));
        for expired in &expired {
            let request = peer.next().await;
            assert_eq!(request.method, reqwest::Method::DELETE);
            let query: std::collections::BTreeMap<_, _> =
                url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(query["key"], expired.object_key());
            assert_eq!(query["mutationGeneration"], "2");
            request.respond(404, "");
        }
        let request = peer.next().await;
        assert_eq!(request.method, reqwest::Method::DELETE);
        let query: std::collections::BTreeMap<_, _> =
            url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(query["key"], key);
        assert_eq!(query["mutationGeneration"], "2");
        request.json(serde_json::json!({ "data": { "deleted": true, "key": key,
            "generation": 2, "fenceVersion": 1 } }));
    };
    let (pass, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(worker.run_once(), responses)
    })
    .await
    .unwrap();
    pass.unwrap();
    assert_eq!(usage(&quota, "owner").await, [4, 16, 4]);
    for source in &expired {
        let charge = object_row(source).unwrap();
        assert_eq!(
            quota
                .rows
                .find_one(doc! { "_id": &charge["_id"] })
                .await
                .unwrap(),
            Some(charge)
        );
    }

    // Finish the cursor wrap and reread the protected first page.
    worker.run_once().await.unwrap();
    worker.run_once().await.unwrap();
    worker.run_once().await.unwrap();
    assert_eq!(peer.count(), 2 + expired.len());
    for live in created {
        assert_eq!(
            service
                .source(&owner, &live.id, None, None)
                .await
                .unwrap()
                .markdown,
            "body"
        );
    }
    database.drop().await.unwrap();
}

async fn seed_expired_claims(quota: &MongoDocumentQuotaStore) -> Vec<DocumentSourceReservation> {
    use crate::document_artifacts::object_authority::{self, Authority, Lease};

    let mut sources = Vec::new();
    for id in 1..=4 {
        let mut source = source("indexed-fixture", id, b"pending");
        source.owner_hash = "f".repeat(64);
        quota.reserve(&source).await.unwrap();
        let key = object_authority::key(&source);
        let encoded = quota
            .rows
            .find_one(doc! { "_id": &key })
            .await
            .unwrap()
            .unwrap();
        let mut authority = Authority::decode(&encoded).unwrap();
        authority
            .claim_delete(
                Lease {
                    holder: digest(b"abandoned"),
                    until: DateTime::from_millis(2),
                },
                DateTime::from_millis(1),
            )
            .unwrap();
        quota
            .rows
            .replace_one(doc! { "_id": &key }, authority.encoded())
            .await
            .unwrap();
        sources.push(source);
    }
    sources
}

fn namespace() -> String {
    digest(b"isolated-reclamation-provider")
}

use super::*;
use crate::document_artifacts::{
    reclamation_store::{DeleteClaim, ReclamationStore},
    MongoDocumentStore,
};
use crate::document_storage::maintenance::ConfirmedDeletion;
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use mongodb::bson::DateTime;

fn confirmed(claim: &DeleteClaim) -> ConfirmedDeletion {
    ConfirmedDeletion::fixture(claim.source.object_key(), claim.generation, namespace())
}

#[path = "reclamation_worker_tests.rs"]
mod worker;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_preserves_charge_until_the_claim_is_confirmed() {
    let database = database().await;
    let store = MongoDocumentStore::new(database.clone()).with_storage_namespace(Some(namespace()));
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    let source = source("owner", 1, b"body");
    quota.reserve(&source).await.unwrap();
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    let reclaimer = ReclamationStore::new(store.clone(), namespace());
    reclaimer.ensure_indexes().await.unwrap();
    reclaimer.enable_generations().await.unwrap();

    let mismatched = ReclamationStore::new(store.clone(), digest(b"other-storage"));
    assert_eq!(
        mismatched.enable_generations().await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(
        quota
            .bind_storage(
                &baseline.activation,
                &baseline.digest,
                &digest(b"other-storage")
            )
            .await,
        Err(DocumentError::Conflict)
    );
    let now = DateTime::now();
    let discovered = reclaimer.accounting_page(None, now, true).await.unwrap();
    let [candidate] = discovered.as_slice() else {
        panic!("expected one legacy reservation");
    };
    let first = reclaimer
        .claim(candidate.candidate.as_ref().unwrap(), now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    assert_eq!(first.generation, 2);
    assert_eq!(
        reclaimer
            .settle(
                &first,
                ConfirmedDeletion::fixture(
                    first.source.object_key(),
                    first.generation,
                    digest(b"other-storage")
                )
            )
            .await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);

    // The first effect timed out. A new process sees its original charge and
    // acquires the persisted delete generation after expiration.
    let restarted = ReclamationStore::new(store, namespace());
    assert!(restarted.expired_claims(now).await.unwrap().is_empty());
    let after_expiry = DateTime::from_millis(now.timestamp_millis() + 91_000);
    let due = restarted.expired_claims(after_expiry).await.unwrap();
    let [candidate] = due.as_slice() else {
        panic!("expected one expired claim");
    };
    let successor = restarted
        .claim(candidate, after_expiry)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.generation, first.generation);
    assert_eq!(
        restarted.settle(&first, confirmed(&first)).await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);

    restarted
        .settle(&successor, confirmed(&successor))
        .await
        .unwrap();
    // The legacy reservation has no evidence of the Document's publication history.
    assert_eq!(usage(&quota, "owner").await, [1, 0, 0]);
    assert_eq!(
        restarted.settle(&successor, confirmed(&successor)).await,
        Err(DocumentError::Conflict)
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_rejects_a_delayed_publication_and_preserves_recreation() {
    let database = database().await;
    let authored = Arc::new(memory::MemoryDocuments::default());
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let created = DocumentService::new(
        authored.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    )
    .create(
        &owner,
        "create",
        CreateDocument {
            title: "Document".into(),
            markdown: "body".into(),
            origin_agent_id: None,
        },
    )
    .await
    .unwrap();
    let record = authored.get(&created.id).await.unwrap().unwrap();
    let source = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: created.id.clone(),
        sha256: created.sha256,
        bytes: created.size_bytes,
    };

    let writer = MongoDocumentStore::new(database.clone())
        .with_storage_namespace(Some(namespace()))
        .with_generation_writes(true);
    writer.ensure_indexes().await.unwrap();
    let quota = writer.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    let first = DocumentSourceWrite {
        source: source.clone(),
        fence: quota.reserve_source(&source).await.unwrap(),
    };
    assert_eq!(first.generation(), 1);
    let reclaimer = ReclamationStore::new(writer.clone(), namespace());
    let aged = DateTime::from_millis(DateTime::now().timestamp_millis() + 7_200_000);
    let due = reclaimer.accounting_page(None, aged, false).await.unwrap();
    let [candidate] = due.as_slice() else {
        panic!("expected one stale source reservation");
    };
    let deletion = reclaimer
        .claim(candidate.candidate.as_ref().unwrap(), aged)
        .await
        .unwrap()
        .unwrap();

    let restarted = writer.with_generation_writes(false);
    assert_eq!(
        restarted.insert_source(&record, &first).await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        restarted.quota(true).reserve_source(&source).await,
        Err(DocumentError::Unavailable)
    );
    assert!(restarted.get(&created.id).await.unwrap().is_none());
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    reclaimer
        .settle(&deletion, confirmed(&deletion))
        .await
        .unwrap();
    assert_eq!(usage(&quota, "owner").await, [0, 0, 0]);

    let retry = DocumentSourceWrite {
        source: source.clone(),
        fence: restarted.quota(true).reserve_source(&source).await.unwrap(),
    };
    assert!(retry.generation() > deletion.generation);
    assert_ne!(retry.fence, first.fence);
    assert!(restarted.insert_source(&record, &retry).await.unwrap());
    assert_eq!(
        reclaimer.settle(&deletion, confirmed(&deletion)).await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        restarted.get(&created.id).await.unwrap().unwrap().metadata,
        record.metadata
    );
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_serializes_last_refund_with_a_new_digest() {
    let database = database().await;
    let store = MongoDocumentStore::new(database.clone())
        .with_storage_namespace(Some(namespace()))
        .with_generation_writes(true);
    store.ensure_indexes().await.unwrap();
    let quota = store.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    let old = source("owner", 1, b"first");
    let new = source("owner", 1, b"another");
    quota.reserve(&old).await.unwrap();
    let reclaimer = ReclamationStore::new(store, namespace());
    let aged = DateTime::from_millis(DateTime::now().timestamp_millis() + 7_200_000);
    let due = reclaimer.accounting_page(None, aged, false).await.unwrap();
    let [candidate] = due.as_slice() else {
        panic!("expected one stale source reservation");
    };
    let deletion = reclaimer
        .claim(candidate.candidate.as_ref().unwrap(), aged)
        .await
        .unwrap()
        .unwrap();
    let (settled, admitted) = tokio::join!(
        reclaimer.settle(&deletion, confirmed(&deletion)),
        quota.reserve(&new)
    );
    settled.unwrap();
    admitted.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 7, 1]);
    quota.reserve(&new).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 7, 1]);
    assert_eq!(
        reclaimer.settle(&deletion, confirmed(&deletion)).await,
        Err(DocumentError::Conflict)
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_reclamation_upgrades_a_marker_for_an_identical_source_edit() {
    let database = database().await;
    let predecessor = MongoDocumentStore::new(database.clone())
        .with_storage_namespace(Some(namespace()))
        .with_source_commits(true);
    predecessor.ensure_indexes().await.unwrap();
    let quota = predecessor.quota(true);
    let baseline = activate(&quota, &database, &[]).await;
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let previous = DocumentService::with_storage_quota(
        Arc::new(predecessor.clone()),
        blobs.clone(),
        false,
        Some(Arc::new(predecessor.quota(true))),
    );
    let created = previous
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Original".into(),
                markdown: "body".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    quota
        .bind_storage(&baseline.activation, &baseline.digest, &namespace())
        .await
        .unwrap();
    let first = predecessor.get(&created.id).await.unwrap().unwrap();
    let source = DocumentSourceReservation {
        owner_hash: digest(b"owner"),
        document_id: created.id.clone(),
        sha256: created.sha256.clone(),
        bytes: created.size_bytes,
    };
    let reclaimer = ReclamationStore::new(predecessor.clone(), namespace());
    let aged = DateTime::from_millis(DateTime::now().timestamp_millis() + 7_200_000);
    let candidates = reclaimer.accounting_page(None, aged, true).await.unwrap();
    let [legacy] = candidates.as_slice() else {
        panic!("expected one legacy marker");
    };

    let writer = predecessor.with_generation_writes(true);
    let documents = DocumentService::with_storage_quota(
        Arc::new(writer.clone()),
        blobs,
        false,
        Some(Arc::new(writer.quota(true))),
    );
    let updated = documents
        .update(
            &owner,
            &created.id,
            "edit",
            UpdateDocument {
                expected_revision: 1,
                title: Some("Updated".into()),
                markdown: Some("body".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert!(reclaimer
        .claim(legacy.candidate.as_ref().unwrap(), aged)
        .await
        .unwrap()
        .is_none());
    assert_eq!(usage(&quota, "owner").await, [1, 4, 1]);
    let marker = quota
        .rows
        .find_one(doc! { "_id": crate::document_artifacts::object_authority::key(&source) })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marker.get_i64("fence").unwrap(), 2);
    assert_eq!(marker.get_str("state").unwrap(), "published");
    assert!(
        !crate::document_artifacts::source_commit::reconcile_after_majority_read(
            &writer, &first, &source, None,
        )
        .await
        .unwrap()
    );
    database.drop().await.unwrap();
}

#[path = "reclamation_fences_tests.rs"]
mod fences;

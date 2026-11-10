use super::MongoDocumentStore;
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use mongodb::{
    bson::{doc, Document},
    Client,
};
use std::sync::Arc;
mod rollout;
mod tenant_isolation;
fn caller(s: &str) -> Caller {
    Caller::new(
        SubjectId::new(s),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn mongo_documents_persist_isolated_metadata_and_fence_concurrent_edits() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_documents_test_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let store = Arc::new(MongoDocumentStore::new(database.clone()));
    store.ensure_indexes().await.unwrap();
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::new(store.clone(), blobs.clone());
    let owner = caller("document-owner");
    let reader = caller("document-reader");
    let document = service
        .create(
            &owner,
            "create",
            CreateDocument {
                title: "Exact $literal.* report".into(),
                markdown: "# Source stays outside Mongo".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let raw = database
        .collection::<Document>(super::COLLECTION)
        .find_one(doc! {"_id":&document.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("document_schema_version").unwrap(), 1);
    assert!(!format!("{raw:?}").contains("Source stays outside Mongo"));
    let update = UpdateDocument {
        expected_revision: 1,
        title: Some("Winning title".into()),
        markdown: None,
    };
    let (a, b) = tokio::join!(
        service.update(&owner, &document.id, "edit-a", update.clone()),
        service.update(&owner, &document.id, "edit-b", update)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(DocumentError::Conflict)) || matches!(b, Err(DocumentError::Conflict)));
    // A fresh adapter emulates control-plane replacement; no in-process state
    // determines ownership, current source, revisions, receipts or permissions.
    let reopened = Arc::new(MongoDocumentStore::new(database.clone()));
    reopened.ensure_indexes().await.unwrap();
    let service = DocumentService::new(reopened, blobs);
    assert_eq!(
        service
            .read(&owner, &document.id, None)
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        service
            .source(&owner, &document.id, None, Some(1))
            .await
            .unwrap()
            .markdown,
        "# Source stays outside Mongo"
    );
    assert!(service
        .list(&reader, &DocumentQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    service
        .grant(
            &owner,
            &document.id,
            "share",
            reader.subject.as_str(),
            GrantDocument {
                expected_revision: 2,
                permission: DocumentPermission::Comment,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .list(&reader, &DocumentQuery::default())
            .await
            .unwrap()
            .items[0]
            .access,
        DocumentAccess::Comment
    );
    let posted = service
        .comment(
            &reader,
            &document.id,
            None,
            "comment",
            CreateDocumentComment {
                body: "Shared comment".into(),
                document_revision: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(posted.document.revision, 4);
    service
        .revoke(&owner, &document.id, "revoke", reader.subject.as_str(), 4)
        .await
        .unwrap();
    assert_eq!(
        service.source(&reader, &document.id, None, None).await,
        Err(DocumentError::NotFound)
    );
    assert!(service
        .list(&reader, &DocumentQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let listed = service
        .list(
            &owner,
            &DocumentQuery {
                q: Some("winning".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert!(service
        .list(
            &owner,
            &DocumentQuery {
                q: Some(".*".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .items
        .is_empty());
    database.drop().await.unwrap();
}

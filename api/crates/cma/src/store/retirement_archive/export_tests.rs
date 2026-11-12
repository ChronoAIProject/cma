use super::*;
use crate::retirement_archive::tests::{request, Custody, Temp};
use layer2_core::document_artifacts::{memory, CreateDocument, DocumentService, DocumentStore};
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;

#[tokio::test]
async fn external_document_source_is_retained_and_missing_corrupt_future_sources_refuse() {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::new(rows.clone(), blobs.clone());
    let caller = Caller::new(
        SubjectId::new("fixture-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let view = service
        .create(
            &caller,
            "archive-fixture",
            CreateDocument {
                title: "Retained source".into(),
                markdown: "# Retained source bytes".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    let record = rows.get(&view.id).await.unwrap().unwrap();
    let mut document = doc! {"_id":&view.id,"document_schema_version":1_i64,"record":mongodb::bson::serialize_to_bson(&record).unwrap()};
    let temp = Temp::new();
    let request = request(&temp);
    let custody = Custody::default();
    let objects = document_objects(&document, &custody, Some(blobs.as_ref()), &request)
        .await
        .unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(
        custody.read(&objects[0].object).await.unwrap(),
        b"# Retained source bytes"
    );
    assert!(document_objects(&document, &custody, None, &request)
        .await
        .is_err());
    document.insert("document_schema_version", 2_i64);
    assert!(
        document_objects(&document, &custody, Some(blobs.as_ref()), &request)
            .await
            .is_err()
    );
    document.insert("document_schema_version", 1_i64);
    blobs
        .0
        .lock()
        .await
        .insert(record.revisions[0].object_key.clone(), b"corrupt".to_vec());
    assert!(
        document_objects(&document, &custody, Some(blobs.as_ref()), &request)
            .await
            .is_err()
    );
    blobs.0.lock().await.clear();
    assert!(
        document_objects(&document, &custody, Some(blobs.as_ref()), &request)
            .await
            .is_err()
    );
}

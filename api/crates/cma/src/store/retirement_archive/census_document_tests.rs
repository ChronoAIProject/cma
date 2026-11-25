use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn document_origin_annotation_cannot_archive_another_accounts_body() {
    use layer2_core::document_artifacts::{
        memory, CreateDocument, DocumentDeletionReceipt, DocumentService, DocumentStore,
        DocumentTombstone,
    };
    use layer2_core::sandbox::store::{AgentAdmissionStore, WorkspaceStore};
    use layer2_core::{Authorization, Caller, Capability, SubjectId};
    use std::{collections::BTreeMap, sync::Arc};
    let (client, name, ordinary, access) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let database = client.database(&name);
    // Admit a canonical Agent ID through the real store: the access fixture's
    // shorthand identity is deliberately invalid at the public document API.
    let workspace = ordinary
        .get_workspace(&access.requester.creation.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut admission =
        crate::store::agent_access_tools::fixtures::admission(&workspace, "archive-document");
    admission.agent_id = layer2_core::AgentId(format!("agt_{}", uuid::Uuid::new_v4().simple()));
    let origin = admission.agent_id.to_string();
    ordinary.admit_agent(admission).await.unwrap();
    let fixture = support::prepare_source(
        client,
        database.clone(),
        access.requester.sandbox_id.as_str(),
    )
    .await;
    let documents = Arc::new(crate::document_artifacts::MongoDocumentStore::new(
        database.clone(),
    ));
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::new(documents.clone(), blobs.clone());
    let owner = Caller::new(
        access.requester.owner,
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let source = service
        .create(
            &owner,
            "source",
            CreateDocument {
                title: "Retained source".into(),
                markdown: "# Source body".into(),
                origin_agent_id: Some(origin.clone()),
            },
        )
        .await
        .unwrap();
    let foreign = Caller::new(
        SubjectId::new("unrelated-account"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let foreign_document = service
        .create(
            &foreign,
            "foreign",
            CreateDocument {
                title: "Unrelated body".into(),
                markdown: "# Never archive this account".into(),
                origin_agent_id: Some(origin.clone()),
            },
        )
        .await
        .unwrap();
    let deleted = service
        .create(
            &owner,
            "deleted",
            CreateDocument {
                title: "Deleted metadata".into(),
                markdown: "# Already deleted".into(),
                origin_agent_id: Some(origin),
            },
        )
        .await
        .unwrap();
    let collection = database.collection::<Document>("cma_document_artifacts");
    for id in [&foreign_document.id, &deleted.id] {
        let audit = documents.get(id).await.unwrap().unwrap();
        let tombstone = DocumentTombstone {
            owner_hash: layer2_core::document_artifacts::digest(
                audit.metadata.owner_subject.as_bytes(),
            ),
            request_hash: layer2_core::document_artifacts::digest(b"delete"),
            expected_revision: audit.metadata.revision,
            receipt: DocumentDeletionReceipt {
                id: id.clone(),
                revision: audit.metadata.revision + 1,
                deleted_at: layer2_core::now_rfc3339(),
            },
        };
        collection
            .replace_one(
                doc! {"_id":id},
                crate::document_artifacts::deletion::encode(&tombstone, &audit).unwrap(),
            )
            .await
            .unwrap();
        blobs.0.lock().await.remove(&audit.revisions[0].object_key);
    }
    collection
        .insert_one(doc! {"_id":"unknown", "document_schema_version":3_i64})
        .await
        .unwrap();
    let mut snapshot = database
        .client()
        .start_session()
        .snapshot(true)
        .await
        .unwrap();
    let closure = census::identities(&database, &fixture.2.sandbox_id, &mut snapshot)
        .await
        .unwrap();
    assert!(
        census::selection(&database, "cma_document_artifacts", &closure, &mut snapshot)
            .await
            .is_err()
    );
    drop(snapshot);
    collection.delete_one(doc! {"_id":"unknown"}).await.unwrap();
    let mut ids = [source.id, deleted.id];
    ids.sort();
    let mut expected_bytes = Vec::new();
    for id in ids {
        let row = database
            .collection::<mongodb::bson::RawDocumentBuf>("cma_document_artifacts")
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap();
        expected_bytes.extend(row.as_bytes());
    }
    let expected = BTreeMap::from([("cma_document_artifacts".into(), expected_bytes)]);
    support::roundtrip_with_blobs(fixture, expected, Some(blobs.as_ref())).await;
}

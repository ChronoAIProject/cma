use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn read(app: &Router, who: &Caller, path: &str, token: Option<&str>) -> (u16, Value) {
    let mut request = Request::get(path);
    if let Some(token) = token {
        request = request.header("x-cma-document-share-token", token);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(who.subject.clone()));
    request.extensions_mut().insert(who.authorization.clone());
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable MongoDB replica set"]
async fn tenant_isolation_revoked_grant_and_link_receipts_do_not_restore_http_access() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!(
        "document_isolation_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(MongoDocumentStore::new(database.clone()));
    store.ensure_indexes().await.unwrap();
    let blobs = Arc::new(memory::MemoryDocumentBlobs::default());
    let service = DocumentService::new(store.clone(), blobs.clone());
    let owner = caller("owner");
    let reader = caller("reader");
    let foreign = caller("foreign");
    for link in [false, true] {
        let d = service
            .create(
                &owner,
                if link {
                    "link-document"
                } else {
                    "grant-document"
                },
                CreateDocument {
                    title: "Private Mongo fixture".into(),
                    markdown: "private-source-sentinel".into(),
                    origin_agent_id: None,
                },
            )
            .await
            .unwrap();
        let grant = GrantDocument {
            expected_revision: 1,
            permission: DocumentPermission::Comment,
        };
        let token = "a1".repeat(32);
        let link_input = CreateDocumentLink {
            expected_revision: 1,
            permission: DocumentPermission::Comment,
            token: token.clone(),
        };
        let granted = if link {
            service
                .create_link(&owner, &d.id, "grant", link_input.clone())
                .await
                .unwrap()
                .link
        } else {
            service
                .grant(
                    &owner,
                    &d.id,
                    "grant",
                    reader.subject.as_str(),
                    grant.clone(),
                )
                .await
                .unwrap();
            None
        };
        let base = format!("/api/v1/documents/{}", d.id);
        let app = layer1_public::rest::documents_router(service.clone());
        let token = link.then_some(token.as_str());
        for suffix in [
            "",
            "/source",
            "/source?revision=1",
            "/revisions",
            "/comments",
        ] {
            assert_eq!(
                read(&app, &reader, &format!("{base}{suffix}"), token)
                    .await
                    .0,
                200
            );
            let denied = read(&app, &foreign, &format!("{base}{suffix}"), None).await;
            assert_eq!(denied.0, 404);
            assert_eq!(denied.1["code"], "document_not_found");
        }
        let revoked = if link {
            service
                .revoke_link(&owner, &d.id, "revoke", &granted.as_ref().unwrap().id, 2)
                .await
                .unwrap()
        } else {
            service
                .revoke(&owner, &d.id, "revoke", reader.subject.as_str(), 2)
                .await
                .unwrap()
        };
        let reopened = Arc::new(MongoDocumentStore::new(database.clone()));
        let reopened_service = DocumentService::new(reopened.clone(), blobs.clone());
        let before = serde_json::to_value(reopened.get(&d.id).await.unwrap().unwrap()).unwrap();
        // Exact old grant retry is historical receipt replay, never a fresh policy write.
        if link {
            reopened_service
                .create_link(&owner, &d.id, "grant", link_input)
                .await
                .unwrap();
            let retry = reopened_service
                .revoke_link(&owner, &d.id, "revoke", &granted.unwrap().id, 2)
                .await
                .unwrap();
            assert_eq!(retry.revision, revoked.revision);
        } else {
            reopened_service
                .grant(&owner, &d.id, "grant", reader.subject.as_str(), grant)
                .await
                .unwrap();
            let retry = reopened_service
                .revoke(&owner, &d.id, "revoke", reader.subject.as_str(), 2)
                .await
                .unwrap();
            assert_eq!(retry.revision, revoked.revision);
        }
        let app = layer1_public::rest::documents_router(reopened_service.clone());
        for suffix in [
            "",
            "/source",
            "/source?revision=1",
            "/revisions",
            "/comments",
            "/shares",
            "/share-links",
        ] {
            assert_eq!(
                read(&app, &owner, &format!("{base}{suffix}"), None).await.0,
                200
            );
            for (who, credential) in [(&reader, token), (&foreign, None)] {
                let denied = read(&app, who, &format!("{base}{suffix}"), credential).await;
                let absent = read(
                    &app,
                    who,
                    &format!("/api/v1/documents/doc_00000000000000000000000000000000{suffix}"),
                    credential,
                )
                .await;
                assert_eq!(denied.0, 404);
                assert_eq!(denied.1["code"], "document_not_found");
                assert_eq!(denied, absent);
            }
        }
        assert_eq!(
            read(&app, &reader, "/api/v1/documents", token).await.1["items"],
            json!([])
        );
        assert!(matches!(
            reopened_service
                .comment(
                    &reader,
                    &d.id,
                    token,
                    "stale-comment",
                    CreateDocumentComment {
                        body: "must not persist".into(),
                        document_revision: Some(1),
                    }
                )
                .await,
            Err(DocumentError::NotFound)
        ));
        assert_eq!(
            serde_json::to_value(reopened.get(&d.id).await.unwrap().unwrap()).unwrap(),
            before
        );
    }
    database.drop().await.unwrap();
}

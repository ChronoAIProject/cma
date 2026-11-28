use super::*;
use layer2_core::sandbox::HierarchyReadStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_sandbox_quarantine_keeps_owner_http_inventory_available() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use layer1_public::{
        rest::{hierarchy_router, HierarchyState},
        AuthenticatedSubject,
    };
    use layer2_core::{
        profiles::store::InMemoryProfileStore,
        sandbox::{
            FixedSandboxCredentialPrerequisite, HierarchyReadService, SandboxPolicy,
            SandboxService, WorkspaceService,
        },
        Authorization, Capability,
    };
    use tower::ServiceExt as _;

    let (client, name) = database("quarantined_owner_inventory").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    for id in [
        "sbx_a_marked",
        "sbx_b_healthy",
        "sbx_c_marked",
        "sbx_d_healthy",
        "sbx_e_marked",
    ] {
        let mut value = sandbox(id);
        value.name = id.into();
        store.create_sandbox(value).await.unwrap();
        if id.ends_with("_marked") {
            store
                .sandboxes
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{(crate::store::sandbox_quarantine::FIELD):{}}},
                )
                .await
                .unwrap();
        }
    }
    let profiles = Arc::new(InMemoryProfileStore::default());
    let operations = Arc::new(
        crate::store::operation::MongoOperationStore::open(&client, &name)
            .await
            .unwrap(),
    );
    let router = hierarchy_router(HierarchyState::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(store.clone(), profiles),
        HierarchyReadService::new(store),
        operations,
    ));
    let get = |uri: &str| {
        let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new("owner")));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
        router.clone().oneshot(request)
    };
    for (path, expected, next) in [
        (
            "/api/v1/sandboxes?limit=1",
            "sbx_b_healthy",
            Some("sbx_b_healthy"),
        ),
        (
            "/api/v1/sandboxes?limit=1&cursor=sbx_b_healthy",
            "sbx_d_healthy",
            None,
        ),
    ] {
        let response = get(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], expected);
        assert_eq!(body["next"].as_str(), next);
    }
    let response = get("/api/v1/sandboxes/sbx_a_marked").await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn visible_inventory_filters_retired_records_before_counts_and_lookahead() {
    let (client, name) = database("visible_inventory").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for id in ["sbx_00", "sbx_02", "sbx_04"] {
        let mut value = retired_sandbox(id);
        value.name = id.into();
        store.create_sandbox(value).await.unwrap();
    }
    let first = sandbox("sbx_01");
    let mut last = sandbox("sbx_03");
    last.name = "last".into();
    last.status = SandboxStatus::Retiring;
    last.work = SandboxWorkState::new(SandboxWorkStep::StopAgents, NOW.into());
    store.create_sandbox(first.clone()).await.unwrap();
    store.create_sandbox(last.clone()).await.unwrap();
    let page = store
        .list_owner_sandbox_inventory(
            &first.owner,
            None,
            true,
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].sandbox, first);
    assert_eq!(page.next.as_deref(), Some("sbx_01"));
    let page = store
        .list_owner_sandbox_inventory(
            &first.owner,
            None,
            true,
            &PageRequest::new(1, page.next).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].sandbox, last);
    assert_eq!(page.next, None);
    assert_eq!(
        store
            .list_owner_sandbox_inventory(&first.owner, None, false, &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        5
    );
    assert!(store
        .list_owner_sandbox_inventory(
            &first.owner,
            Some(SandboxStatus::Retired),
            true,
            &PageRequest::default()
        )
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        store
            .get_sandbox(&SandboxId("sbx_00".into()))
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Retired
    );
    // Nonretired lookahead remains a fully validated aggregate.
    store
        .sandboxes
        .update_one(
            doc! {"_id":last.id.as_str()},
            doc! {"$set":{"record.owner":"other-owner"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .list_owner_sandbox_inventory(
                &first.owner,
                None,
                true,
                &PageRequest::new(1, None).unwrap()
            )
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

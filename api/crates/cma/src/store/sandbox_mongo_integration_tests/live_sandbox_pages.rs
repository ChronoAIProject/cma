use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_sandbox_pages_filter_retirement_and_owner_before_limit_and_lookahead() {
    let (client, name) = database("live_sandbox_pages").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let owner = SubjectId::new("owner");
    for id in ["sbx_00", "sbx_02", "sbx_04", "sbx_06"] {
        store.create_sandbox(retired_sandbox(id)).await.unwrap();
    }
    let mut retiring = sandbox("sbx_03");
    retiring.name = retiring.id.to_string();
    retiring.status = SandboxStatus::Retiring;
    retiring.work = SandboxWorkState::new(SandboxWorkStep::StopAgents, NOW.into());
    store.create_sandbox(retiring).await.unwrap();
    let mut other = sandbox("sbx_05");
    other.owner = SubjectId::new("another-owner");
    store.create_sandbox(other).await.unwrap();
    let mut first = sandbox("sbx_01");
    first.name = first.id.to_string();
    let mut last = provisioning_sandbox("sbx_07");
    last.name = last.id.to_string();
    store.create_sandbox(first.clone()).await.unwrap();
    store.create_sandbox(last.clone()).await.unwrap();

    let page = store
        .list_live_sandboxes(&owner, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items, [first.clone()]);
    assert_eq!(page.next, Some(first.id.to_string()));
    let page = store
        .list_live_sandboxes(&owner, &PageRequest::new(1, page.next).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items, [last]);
    assert_eq!(
        page.next, None,
        "retired and other-owner records cannot supply lookahead"
    );
    assert_eq!(
        store
            .list_sandboxes(&owner, &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        7,
        "the complete retained inventory stays available"
    );
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert!(matches!(
            store
                .list_live_sandboxes(
                    &owner,
                    &PageRequest {
                        limit,
                        cursor: None
                    }
                )
                .await,
            Err(SandboxStoreError::InvalidPage)
        ));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_sandbox_pages_validate_complete_records_and_hidden_lookahead() {
    let (client, name) = database("live_sandbox_corruption").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let first = sandbox("sbx_a");
    let mut second = sandbox("sbx_b");
    second.name = second.id.to_string();
    store.create_sandbox(first.clone()).await.unwrap();
    store.create_sandbox(second.clone()).await.unwrap();
    // A field unrelated to colour must still be validated, even on lookahead.
    store
        .sandboxes
        .update_one(
            doc! {"_id": second.id.as_str()},
            doc! {"$set": {"record.owner": "wrong-owner"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .list_live_sandboxes(&first.owner, &PageRequest::new(1, None).unwrap())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store.sandboxes.update_one(doc! {"_id": second.id.as_str()},
        doc! {"$set": {"record.owner": first.owner.as_str(), "record.limits.max_workspaces": 0}}).await.unwrap();
    assert!(matches!(
        store
            .list_live_sandboxes(&first.owner, &PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

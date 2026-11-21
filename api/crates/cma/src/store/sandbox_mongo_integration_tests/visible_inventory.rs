use super::*;
use layer2_core::sandbox::HierarchyReadStore;

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

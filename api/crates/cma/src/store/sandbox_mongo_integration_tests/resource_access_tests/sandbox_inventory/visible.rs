use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_sandbox_quarantine_keeps_org_and_shared_inventory_available() {
    let (client, name) = database("quarantined_shared_inventory").await;
    let (store, _) = fixture(&client, &name).await;
    for id in ["a_quarantined", "b_healthy", "c_healthy"] {
        let mut root = sandbox(id);
        root.name = id.into();
        store.create_sandbox(root.clone()).await.unwrap();
        grant(
            &store,
            &ResourceTarget::Sandbox {
                sandbox_id: root.id,
            },
            id,
        )
        .await;
    }
    store
        .sandboxes
        .update_one(
            doc! {"_id": "a_quarantined"},
            doc! {"$set": { (crate::store::sandbox_quarantine::FIELD): {} }},
        )
        .await
        .unwrap();
    let org = SubjectId::new("org");
    let page = query(1, None);
    let owned = store.organization_sandboxes(&org, &page).await.unwrap();
    let shared = store
        .shared_sandboxes(&SubjectId::new("recipient"), &[org], &page)
        .await
        .unwrap();
    assert_eq!(ids(&owned), ["b_healthy", "c_healthy"]);
    assert_eq!(ids(&shared), ["b_healthy", "c_healthy"]);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn visible_inventory_filters_org_and_shared_candidates_before_hidden_lookahead() {
    let (client, name) = database("visible_shared_inventory").await;
    let (store, _) = fixture(&client, &name).await;
    for id in ["a_retired", "b_live", "c_retired", "d_live", "e_retired"] {
        let mut root = sandbox(id);
        root.name = id.into();
        store.create_sandbox(root.clone()).await.unwrap();
        grant(
            &store,
            &ResourceTarget::Sandbox {
                sandbox_id: root.id.clone(),
            },
            id,
        )
        .await;
        if id.ends_with("retired") {
            root.status = SandboxStatus::Retired;
            root.credential.delivered = false;
            root.credential.current = None;
            root.work = SandboxWorkState::new(SandboxWorkStep::Purge, NOW.into());
            root.retired_at = Some(NOW.into());
            root.purge_after = Some(NOW.into());
            root.revision += 1;
            store.save_sandbox(1, root).await.unwrap();
        }
    }
    let org = SubjectId::new("org");
    let recipient = SubjectId::new("recipient");
    let visible = OwnerSandboxQuery {
        exclude_retired: true,
        ..query(1, None)
    };
    for shared in [false, true] {
        let first = if shared {
            store
                .shared_sandboxes(&recipient, std::slice::from_ref(&org), &visible)
                .await
                .unwrap()
        } else {
            store.organization_sandboxes(&org, &visible).await.unwrap()
        };
        assert_eq!(
            ids(&first),
            ["b_live", "d_live"],
            "visible item plus valid hidden lookahead"
        );
        let last = OwnerSandboxQuery {
            page: PageRequest::new(1, Some("b_live".into())).unwrap(),
            ..visible.clone()
        };
        let page = if shared {
            store
                .shared_sandboxes(&recipient, std::slice::from_ref(&org), &last)
                .await
                .unwrap()
        } else {
            store.organization_sandboxes(&org, &last).await.unwrap()
        };
        assert_eq!(
            ids(&page),
            ["d_live"],
            "trailing retired history cannot supply lookahead"
        );
    }
    assert_eq!(
        ids(&store
            .organization_sandboxes(&org, &query(10, None))
            .await
            .unwrap()),
        ["a_retired", "b_live", "c_retired", "d_live", "e_retired"]
    );
    client.database(&name).drop().await.unwrap();
}

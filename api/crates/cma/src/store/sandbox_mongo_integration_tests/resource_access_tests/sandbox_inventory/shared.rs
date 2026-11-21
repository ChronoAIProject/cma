use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_sandbox_inventory_merges_grants_and_excludes_own_and_narrow_roots_before_paging() {
    let (client, name) = database("shared_sandbox_inventory").await;
    let (store, agent_target) = fixture(&client, &name).await;
    let before = store
        .resource_access_snapshot(&agent_target)
        .await
        .unwrap()
        .lineage;
    let logs = store
        .agent_log(&before.agent.as_ref().unwrap().id, 0, 20)
        .await
        .unwrap();
    grant(&store, &agent_target, "narrow").await;
    for (id, owner) in [
        ("a_own", "recipient"),
        ("b_private", "foreign"),
        ("z_shared", "foreign"),
    ] {
        let mut root = sandbox(id);
        root.owner = SubjectId::new(owner);
        root.name = id.into();
        store.create_sandbox(root).await.unwrap();
    }
    for (id, org, key) in [
        ("a_own", "org", "own"),
        ("z_shared", "org-two", "other"),
        ("z_shared", "org", "overlap"),
    ] {
        let target = ResourceTarget::Sandbox {
            sandbox_id: SandboxId(id.into()),
        };
        let snapshot = store.resource_access_snapshot(&target).await.unwrap();
        let mut command = mutation(&snapshot, key, ShareMode::View);
        command.change = ShareChange::Create {
            target_org_id: SubjectId::new(org),
            mode: ShareMode::View,
        };
        store.mutate_share(command).await.unwrap();
    }
    let recipient = SubjectId::new("recipient");
    let orgs = [SubjectId::new("org"), SubjectId::new("org-two")];
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        ids(&two
            .shared_sandboxes(&recipient, &orgs, &query(1, None))
            .await
            .unwrap()),
        ["z_shared"]
    );
    assert!(two
        .shared_sandboxes(&recipient, &[], &query(1, None))
        .await
        .unwrap()
        .is_empty());
    let root = ResourceTarget::Sandbox {
        sandbox_id: before.sandbox.id.clone(),
    };
    grant(&store, &root, "whole").await;
    assert_eq!(
        ids(&two
            .shared_sandboxes(&recipient, &orgs, &query(1, None))
            .await
            .unwrap()),
        ["sbx_shares", "z_shared"]
    );
    assert_eq!(
        ids(&two
            .shared_sandboxes(&recipient, &orgs, &query(1, Some("sbx_shares")))
            .await
            .unwrap()),
        ["z_shared"]
    );
    assert!(two
        .shared_sandboxes(
            &recipient,
            &orgs,
            &OwnerSandboxQuery {
                status: Some(SandboxStatus::Paused),
                ..query(1, None)
            }
        )
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        two.resource_access_snapshot(&agent_target)
            .await
            .unwrap()
            .lineage,
        before
    );
    assert_eq!(
        two.agent_log(&before.agent.as_ref().unwrap().id, 0, 20)
            .await
            .unwrap(),
        logs
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_sandbox_inventory_rejects_corrupt_lookahead_and_reopens_after_revocation() {
    let (client, name) = database("shared_sandbox_corrupt").await;
    let (store, agent_target) = fixture(&client, &name).await;
    let before = store
        .resource_access_snapshot(&agent_target)
        .await
        .unwrap()
        .lineage;
    let root = ResourceTarget::Sandbox {
        sandbox_id: before.sandbox.id.clone(),
    };
    grant(&store, &root, "root").await;
    let mut hidden_root = sandbox("z_hidden");
    hidden_root.name = "hidden".into();
    store.create_sandbox(hidden_root).await.unwrap();
    let target = ResourceTarget::Sandbox {
        sandbox_id: SandboxId("z_hidden".into()),
    };
    let hidden = grant(&store, &target, "hidden").await;
    let recipient = SubjectId::new("recipient");
    let orgs = [SubjectId::new("org")];
    store
        .resource_shares
        .update_one(
            doc! {"_id":&hidden.id},
            doc! {"$set":{"record.target_org_id":"forged"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .shared_sandboxes(&recipient, &orgs, &query(1, None))
            .await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    store
        .resource_shares
        .update_one(
            doc! {"_id":&hidden.id},
            doc! {"$set":{"record.target_org_id":"org"}},
        )
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let mut revoke = mutation(&snapshot, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: hidden.id,
    };
    revoke.expected_share_revision = Some(hidden.revision);
    store.mutate_share(revoke).await.unwrap();
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        ids(&two
            .shared_sandboxes(&recipient, &orgs, &query(1, None))
            .await
            .unwrap()),
        ["sbx_shares"]
    );
    assert_eq!(
        two.resource_access_snapshot(&agent_target)
            .await
            .unwrap()
            .lineage,
        before
    );
    client.database(&name).drop().await.unwrap();
}

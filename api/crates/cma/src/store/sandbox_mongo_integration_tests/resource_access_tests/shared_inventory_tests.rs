use super::*;

mod task_status;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_provenance_inventory_merges_orgs_and_excludes_owner_before_paging() {
    let (client, name) = database("shared_provenance_inventory").await;
    let (store, target) = fixture(&client, &name).await;
    for id in ["a-private", "b-other-org", "z-shared"] {
        add_agent(&store, id).await;
    }
    let before = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage;
    for (index, (id, org)) in [
        ("agt_share", "org"),
        ("agt_share", "org-two"),
        ("z-shared", "org-two"),
        ("b-other-org", "unrelated"),
    ]
    .into_iter()
    .enumerate()
    {
        let snapshot = store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: AgentId(id.into()),
            })
            .await
            .unwrap();
        let mut command = mutation(&snapshot, &format!("grant-{index}"), ShareMode::View);
        command.change = ShareChange::Create {
            target_org_id: SubjectId::new(org),
            mode: ShareMode::View,
        };
        store.mutate_share(command).await.unwrap();
    }
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let recipient = SubjectId::new("recipient");
    let organizations = [SubjectId::new("org"), SubjectId::new("org-two")];
    let query = AgentListQuery::default();
    let page = PageRequest::new(1, None).unwrap();
    let first = two
        .scan_shared_agent_provenance_inventory(&recipient, &organizations, &query, &page)
        .await
        .unwrap();
    assert_eq!(
        ids(&first
            .into_iter()
            .map(|row| row.provenance.resource)
            .collect::<Vec<_>>()),
        ["agt_share", "z-shared"]
    );
    let last = two
        .scan_shared_agent_provenance_inventory(
            &recipient,
            &organizations,
            &query,
            &PageRequest::new(1, Some("agt_share".into())).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&last
            .into_iter()
            .map(|row| row.provenance.resource)
            .collect::<Vec<_>>()),
        ["z-shared"]
    );
    assert!(two
        .scan_shared_agent_provenance_inventory(
            &before.sandbox.owner,
            &organizations,
            &query,
            &page
        )
        .await
        .unwrap()
        .is_empty());
    assert!(two
        .scan_shared_agent_provenance_inventory(&recipient, &[], &query, &page)
        .await
        .unwrap()
        .is_empty());
    assert!(two
        .scan_shared_agent_provenance_inventory(
            &recipient,
            &organizations,
            &AgentListQuery {
                status: Some(AgentStatus::Paused),
                ..Default::default()
            },
            &page
        )
        .await
        .unwrap()
        .is_empty());
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Sandbox {
            sandbox_id: before.sandbox.id.clone(),
        })
        .await
        .unwrap();
    store
        .mutate_share(mutation(&snapshot, "whole", ShareMode::View))
        .await
        .unwrap();
    let all = two
        .scan_shared_agent_provenance_inventory(
            &recipient,
            &organizations,
            &query,
            &PageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&all
            .into_iter()
            .map(|row| row.provenance.resource)
            .collect::<Vec<_>>()),
        ["a-private", "agt_share", "b-other-org", "z-shared"]
    );
    assert_eq!(
        store
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_provenance_inventory_rejects_corrupt_lookahead_and_removed_grants() {
    let (client, name) = database("shared_provenance_corrupt").await;
    let (store, target) = fixture(&client, &name).await;
    add_agent(&store, "z-shared").await;
    let mut grants = Vec::new();
    for id in ["agt_share", "z-shared"] {
        let snapshot = store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: AgentId(id.into()),
            })
            .await
            .unwrap();
        grants.push(
            store
                .mutate_share(mutation(&snapshot, id, ShareMode::View))
                .await
                .unwrap()
                .share,
        );
    }
    let recipient = SubjectId::new("recipient");
    let orgs = [SubjectId::new("org")];
    let query = AgentListQuery::default();
    let page = PageRequest::new(1, None).unwrap();
    store
        .resource_shares
        .update_one(
            doc! {"_id":&grants[1].id},
            doc! {"$set":{"record.target_org_id":"forged"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .scan_shared_agent_provenance_inventory(&recipient, &orgs, &query, &page)
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .resource_shares
        .update_one(
            doc! {"_id":&grants[1].id},
            doc! {"$set":{"record.target_org_id":"org"}},
        )
        .await
        .unwrap();
    task_status::assert_authoritative_status_and_ready_evidence(&store, &target).await;
    for (index, grant) in grants.into_iter().enumerate() {
        let grant_target = match &grant.selector {
            ResourceSelector::Agent { agent_id } => ResourceTarget::Agent {
                agent_id: agent_id.clone(),
            },
            _ => unreachable!(),
        };
        let snapshot = store.resource_access_snapshot(&grant_target).await.unwrap();
        let mut revoke = mutation(&snapshot, &format!("revoke-{index}"), ShareMode::View);
        revoke.change = ShareChange::Revoke { share_id: grant.id };
        revoke.expected_share_revision = Some(grant.revision);
        store.mutate_share(revoke).await.unwrap();
    }
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(two
        .scan_shared_agent_provenance_inventory(&recipient, &orgs, &query, &page)
        .await
        .unwrap()
        .is_empty());
    assert!(two.resource_access_snapshot(&target).await.is_ok());
    client.database(&name).drop().await.unwrap();
}

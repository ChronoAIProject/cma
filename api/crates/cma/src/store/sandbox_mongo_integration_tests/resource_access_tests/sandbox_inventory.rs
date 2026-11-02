use super::*;
use layer2_core::sandbox::OwnerSandboxQuery;
#[path = "sandbox_inventory/shared.rs"]
mod shared;

fn query(limit: u16, cursor: Option<&str>) -> OwnerSandboxQuery {
    OwnerSandboxQuery {
        status: None,
        page: PageRequest::new(limit, cursor.map(str::to_owned)).unwrap(),
    }
}
fn ids(values: &[ResourceAccessSnapshot]) -> Vec<&str> {
    values
        .iter()
        .map(|value| value.lineage.sandbox.id.as_str())
        .collect()
}
async fn grant(store: &MongoSandboxStore, target: &ResourceTarget, key: &str) -> ResourceShare {
    let snapshot = store.resource_access_snapshot(target).await.unwrap();
    store
        .mutate_share(mutation(&snapshot, key, ShareMode::View))
        .await
        .unwrap()
        .share
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_inventory_uses_one_snapshot_and_excludes_narrow_grants_before_paging() {
    let (admin, name) = database("sandbox_org_inventory").await;
    let trace = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let (store, agent_target) = fixture(&client, &name).await;
    let retained = store.resource_access_snapshot(&agent_target).await.unwrap();
    let logs = store
        .agent_log(&retained.lineage.agent.as_ref().unwrap().id, 0, 20)
        .await
        .unwrap();
    grant(&store, &agent_target, "narrow").await;
    for id in ["a_private", "z_shared"] {
        let mut root = sandbox(id);
        root.name = id.into();
        store.create_sandbox(root).await.unwrap();
    }
    let other = ResourceTarget::Sandbox {
        sandbox_id: SandboxId("z_shared".into()),
    };
    grant(&store, &other, "whole-other").await;
    let org = SubjectId::new("org");
    assert_eq!(
        ids(&store
            .organization_sandboxes(&org, &query(1, None))
            .await
            .unwrap()),
        ["z_shared"]
    );
    let root = ResourceTarget::Sandbox {
        sandbox_id: retained.lineage.sandbox.id.clone(),
    };
    grant(&store, &root, "whole-root").await;
    grant(&store, &root, "overlapping").await;
    trace.lock().unwrap().clear();
    let page = store
        .organization_sandboxes(&org, &query(1, None))
        .await
        .unwrap();
    assert_eq!(
        ids(&page),
        [retained.lineage.sandbox.id.as_str(), "z_shared"]
    );
    let commands = trace.lock().unwrap().clone();
    let aggregates: Vec<_> = commands
        .iter()
        .filter(|(name, _)| name == "aggregate")
        .collect();
    assert_eq!(
        aggregates.len(),
        2 + page.len(),
        "ownership and whole-parent grants, then each candidate's ancestry"
    );
    assert_eq!(
        aggregates[..2]
            .iter()
            .map(|(_, command)| command.get_str("aggregate").unwrap())
            .collect::<Vec<_>>(),
        [store.resource_policies.name(), store.resource_shares.name()]
    );
    assert!(aggregates[2..]
        .iter()
        .all(|(_, command)| command.get_str("aggregate").unwrap() == store.sandboxes.name()));
    assert!(command_uses_snapshot_read(&aggregates[0].1));
    for (_, command) in &commands {
        assert_eq!(
            command.get_document("lsid").unwrap(),
            aggregates[0].1.get_document("lsid").unwrap()
        );
        assert_eq!(
            command.get_i64("txnNumber").unwrap(),
            aggregates[0].1.get_i64("txnNumber").unwrap()
        );
        assert!(!command.get_bool("autocommit").unwrap());
    }
    assert!(commands.iter().all(
        |(name, _)| ["aggregate", "find", "getMore", "abortTransaction"].contains(&name.as_str())
    ));
    assert_eq!(
        ids(&store
            .organization_sandboxes(&org, &query(1, Some(retained.lineage.sandbox.id.as_str())))
            .await
            .unwrap()),
        ["z_shared"]
    );
    assert!(store
        .organization_sandboxes(&SubjectId::new("other-org"), &query(10, None))
        .await
        .unwrap()
        .is_empty());
    let filtered = OwnerSandboxQuery {
        status: Some(SandboxStatus::Paused),
        ..query(10, None)
    };
    assert!(store
        .organization_sandboxes(&org, &filtered)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .resource_access_snapshot(&agent_target)
            .await
            .unwrap()
            .lineage,
        retained.lineage
    );
    assert_eq!(
        store
            .agent_log(&retained.lineage.agent.as_ref().unwrap().id, 0, 20)
            .await
            .unwrap(),
        logs
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_inventory_rejects_corrupt_lookahead_and_drops_revoked_candidates() {
    let (client, name) = database("sandbox_org_revoked").await;
    let (store, agent_target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&agent_target).await.unwrap();
    let root = ResourceTarget::Sandbox {
        sandbox_id: snapshot.lineage.sandbox.id.clone(),
    };
    grant(&store, &root, "first").await;
    let mut other = sandbox("z_lookahead");
    other.name = "lookahead".into();
    store.create_sandbox(other.clone()).await.unwrap();
    let target = ResourceTarget::Sandbox {
        sandbox_id: other.id,
    };
    let share = grant(&store, &target, "last").await;
    store
        .resource_shares
        .update_one(
            doc! {"_id":&share.id},
            doc! {"$set":{"record.target_org_id":"another-org"}},
        )
        .await
        .unwrap();
    let org = SubjectId::new("org");
    assert_eq!(
        store.organization_sandboxes(&org, &query(1, None)).await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    store
        .resource_shares
        .update_one(
            doc! {"_id":&share.id},
            doc! {"$set":{"record.target_org_id":"org"}},
        )
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let mut command = mutation(&snapshot, "revoke", ShareMode::View);
    command.change = ShareChange::Revoke { share_id: share.id };
    command.expected_share_revision = Some(share.revision);
    store.mutate_share(command).await.unwrap();
    assert_eq!(
        ids(&store
            .organization_sandboxes(&org, &query(1, None))
            .await
            .unwrap()),
        ["sbx_shares"]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_sandbox_inventory_requires_valid_explicit_ownership_evidence() {
    let (client, name) = database("sandbox_org_owner").await;
    let (store, agent_target) = fixture(&client, &name).await;
    let root = store
        .resource_access_snapshot(&agent_target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    assert!(store
        .organization_sandboxes(&root.owner, &query(10, None))
        .await
        .unwrap()
        .is_empty());
    let policy = ResourcePolicyRecord {
        sandbox_id: root.id.clone(),
        sandbox_created_at: root.created_at,
        owner: ResourceOwner::Organization {
            subject: root.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: root.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("fixture-admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    store.resource_policies.insert_one(doc! {"_id":root.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"owner_kind":"organization","owner_subject":root.owner.as_str(),"revision":1_i64,"record":crate::store::sandbox_mongo::record_bson(&policy).unwrap()}).await.unwrap();
    assert_eq!(
        ids(&store
            .organization_sandboxes(&root.owner, &query(10, None))
            .await
            .unwrap()),
        [root.id.as_str()]
    );
    store
        .resource_policies
        .update_one(
            doc! {"_id":root.id.as_str()},
            doc! {"$set":{"record.sandbox_created_at":"2020-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .organization_sandboxes(&root.owner, &query(10, None))
            .await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}

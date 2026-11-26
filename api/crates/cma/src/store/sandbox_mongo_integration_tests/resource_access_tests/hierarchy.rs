use super::*;
use layer2_core::sandbox::{AgentTaskStatus, HierarchyReadService, HierarchyReadStore};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hierarchy_children_use_one_read_only_snapshot_and_preserve_retained_state() {
    let (admin, name) = database("hierarchy_snapshot").await;
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
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let root = &before.lineage.sandbox;
    let workspace = before.lineage.workspace.as_ref().unwrap();
    let agent = before.lineage.agent.as_ref().unwrap();
    let log = store.agent_log(&agent.id, 0, 20).await.unwrap();
    assert_eq!(agent.status, AgentStatus::Ready);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    for sandbox_read in [true, false] {
        trace.lock().unwrap().clear();
        let expected_reads = if sandbox_read {
            let value = store
                .read_sandbox_hierarchy(&root.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(value.sandbox, *root);
            assert_eq!(value.workspaces, std::slice::from_ref(workspace));
            assert_eq!(value.worktrees, [before.lineage.worktree.clone().unwrap()]);
            assert_eq!(value.agents, std::slice::from_ref(agent));
            vec![
                doc! {"find": "sandboxes", "filter": {"_id": root.id.as_str()}, "limit": 1_i64, "singleBatch": true},
                doc! {"find": "workspaces", "filter": {"sandbox_id": root.id.as_str()}, "sort": {"_id": 1}},
                doc! {"find": "worktrees", "filter": {"sandbox_id": root.id.as_str()}, "sort": {"_id": 1}},
                doc! {"find": "agents", "filter": {"sandbox_id": root.id.as_str()}, "sort": {"_id": 1}},
            ]
        } else {
            let value = store
                .read_workspace_hierarchy(&workspace.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(value.workspace, *workspace);
            assert_eq!(value.worktrees, [before.lineage.worktree.clone().unwrap()]);
            assert_eq!(value.agents, std::slice::from_ref(agent));
            assert_eq!(
                value.task_statuses,
                std::collections::HashMap::from([(agent.id.clone(), AgentTaskStatus::NotStarted)])
            );
            vec![
                doc! {"find": "workspaces", "filter": {"_id": workspace.id.as_str()}, "limit": 1_i64, "singleBatch": true},
                doc! {"find": "worktrees", "filter": {"workspace_id": workspace.id.as_str()}, "sort": {"_id": 1}},
                doc! {"find": "agents", "filter": {"workspace_id": workspace.id.as_str()}, "sort": {"_id": 1}},
                doc! {"aggregate": "agents"},
            ]
        };
        let commands = trace.lock().unwrap().clone();
        assert_eq!(
            commands.len(),
            expected_reads.len() + 1,
            "only the specified hierarchy/status/activity reads and abort are allowed"
        );
        let first = &commands[0].1;
        assert!(command_uses_snapshot_read(first));
        assert!(first.get_bool("startTransaction").unwrap());
        for (_, command) in &commands {
            assert_eq!(
                command.get_document("lsid").unwrap(),
                first.get_document("lsid").unwrap()
            );
            assert_eq!(
                command.get_i64("txnNumber").unwrap(),
                first.get_i64("txnNumber").unwrap()
            );
            assert!(!command.get_bool("autocommit").unwrap());
        }
        for (_, command) in &commands[1..] {
            assert!(!command.contains_key("startTransaction"));
            assert!(!command.contains_key("readConcern"));
        }
        for ((command_name, command), expected) in commands.iter().zip(&expected_reads) {
            assert_eq!(
                command_name,
                if expected.contains_key("aggregate") {
                    "aggregate"
                } else {
                    "find"
                }
            );
            assert_eq!(command.get_str("$db").unwrap(), name);
            assert_eq!(command.get_i32("maxTimeMS").unwrap(), 10_000);
            for field in [
                "find",
                "aggregate",
                "filter",
                "sort",
                "limit",
                "singleBatch",
                "projection",
            ] {
                assert_eq!(
                    command.get(field),
                    expected.get(field),
                    "{field}: {command:?}"
                );
            }
        }
        assert_eq!(commands.last().unwrap().0, "abortTransaction");
    }
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        before
    );
    assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap(), log);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_hierarchy_inventory_excludes_organization_before_limit_and_lookahead() {
    let (client, name) = database("hierarchy_personal_page").await;
    let (store, target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let root = snapshot.lineage.sandbox;
    let mut org_root = root.clone();
    org_root.id = SandboxId("zzz_organization".into());
    org_root.name = "Organization fixture".into();
    store.create_sandbox(org_root.clone()).await.unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: org_root.id.clone(),
        sandbox_created_at: org_root.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: org_root.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: org_root.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("fixture-admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    store.resource_policies.insert_one(doc! {"_id":org_root.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"owner_kind":"organization","owner_subject":org_root.owner.as_str(),"revision":1_i64,"record":crate::store::sandbox_mongo::record_bson(&policy).unwrap()}).await.unwrap();
    let page = store
        .list_owner_sandbox_inventory(
            &root.owner,
            None,
            false,
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].sandbox, root);
    assert_eq!(page.items[0].counts.agents, 1);
    assert_eq!(
        page.next, None,
        "organization cannot supply the hidden lookahead"
    );
    // A malformed person index must be decoded even if it is only the lookahead.
    store
        .resource_policies
        .update_one(
            doc! {"_id":org_root.id.as_str()},
            doc! {"$set":{"owner_kind":"person"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .list_owner_sandbox_inventory(
                &root.owner,
                None,
                false,
                &PageRequest::new(1, None).unwrap()
            )
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn hierarchy_rejects_cross_workspace_assignment_without_changing_records() {
    let (client, name) = database("hierarchy_bad_assignment").await;
    let (store, target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let root = &snapshot.lineage.sandbox;
    let other = ready_workspace(root, "wks_other", "other-project");
    store.create_workspace(other.clone()).await.unwrap();
    let tree = snapshot.lineage.worktree.unwrap();
    store.worktrees.update_one(doc! {"_id":tree.id.as_str()}, doc! {"$set":{"workspace_id":other.id.as_str(),"record.workspace_id":other.id.as_str()}}).await.unwrap();
    assert_eq!(
        store
            .resource_access_snapshot(&ResourceTarget::Worktree {
                worktree_id: tree.id.clone()
            })
            .await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    let service = HierarchyReadService::new(Arc::new(store));
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    assert!(matches!(
        service.sandbox(&caller, &root.id).await,
        Err(layer2_core::sandbox::HierarchyReadError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

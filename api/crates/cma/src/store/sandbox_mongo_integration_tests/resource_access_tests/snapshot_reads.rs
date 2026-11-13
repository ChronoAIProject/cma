use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn authorization_ancestry_uses_one_snapshot_query_for_every_target() {
    let (admin, name) = database("authorization_ancestry_reads").await;
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
    let original = store.resource_access_snapshot(&target).await.unwrap();
    let targets = [
        target,
        ResourceTarget::Worktree {
            worktree_id: original.lineage.worktree.as_ref().unwrap().id.clone(),
        },
        ResourceTarget::Workspace {
            workspace_id: original.lineage.workspace.as_ref().unwrap().id.clone(),
        },
        ResourceTarget::Sandbox {
            sandbox_id: original.lineage.sandbox.id.clone(),
        },
    ];
    for target in targets {
        trace.lock().unwrap().clear();
        let current = store.resource_access_snapshot(&target).await.unwrap();
        assert_eq!(current.lineage.sandbox, original.lineage.sandbox);
        assert_eq!(current.policy, original.policy);
        assert_eq!(current.shares, original.shares);
        current.lineage.validate(&target).unwrap();
        let commands = trace.lock().unwrap().clone();
        assert_eq!(
            commands.len(),
            3,
            "ancestry, bounded shares, and abort only"
        );
        assert_eq!(commands[0].0, "aggregate");
        assert_eq!(commands[1].0, "find");
        assert_eq!(commands[2].0, "abortTransaction");
        let first = &commands[0].1;
        assert!(command_uses_snapshot_read(first));
        assert!(first.get_bool("startTransaction").unwrap());
        for (_, command) in &commands[1..] {
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
    }
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn authorization_ancestry_keeps_distinct_record_size_limits() {
    let (client, name) = database("authorization_ancestry_sizes").await;
    let (store, target) = fixture(&client, &name).await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    // Each retained document is legal; the whole ancestry exceeds one BSON row.
    // Unknown retained fields must not change the established record readers.
    let padding = "x".repeat(5 * 1024 * 1024);
    for collection in [
        &store.agents,
        &store.worktrees,
        &store.workspaces,
        &store.sandboxes,
    ] {
        collection
            .update_many(doc! {}, doc! {"$set":{"retained_padding":&padding}})
            .await
            .unwrap();
    }
    for enabled in [false, true] {
        assert_eq!(
            store
                .clone()
                .with_access_snapshot_session(enabled)
                .resource_access_snapshot(&target)
                .await
                .unwrap(),
            original
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn authorization_ancestry_distinguishes_missing_targets_and_invalid_parents() {
    let (client, name) = database("authorization_ancestry_missing").await;
    let (store, target) = fixture(&client, &name).await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    for missing in [
        ResourceTarget::Agent {
            agent_id: AgentId("agt_absent".into()),
        },
        ResourceTarget::Worktree {
            worktree_id: WorktreeId("wkt_absent".into()),
        },
        ResourceTarget::Workspace {
            workspace_id: WorkspaceId("wks_absent".into()),
        },
        ResourceTarget::Sandbox {
            sandbox_id: SandboxId("sbx_absent".into()),
        },
    ] {
        for enabled in [false, true] {
            assert_eq!(
                store
                    .clone()
                    .with_access_snapshot_session(enabled)
                    .resource_access_snapshot(&missing)
                    .await,
                Err(ResourceAccessStoreError::NotFound)
            );
        }
    }
    let worktree = original.lineage.worktree.as_ref().unwrap();
    store
        .worktrees
        .update_one(
            doc! {"_id":worktree.id.as_str()},
            doc! {"$set":{"workspace_id":"wks_wrong"}},
        )
        .await
        .unwrap();
    for enabled in [false, true] {
        assert_eq!(
            store
                .clone()
                .with_access_snapshot_session(enabled)
                .resource_access_snapshot(&target)
                .await,
            Err(ResourceAccessStoreError::Corrupt)
        );
    }
    store
        .worktrees
        .update_one(
            doc! {"_id":worktree.id.as_str()},
            doc! {"$set":{"workspace_id":worktree.workspace_id.as_str()}},
        )
        .await
        .unwrap();
    store
        .workspaces
        .delete_one(doc! {"_id":worktree.workspace_id.as_str()})
        .await
        .unwrap();
    for enabled in [false, true] {
        assert_eq!(
            store
                .clone()
                .with_access_snapshot_session(enabled)
                .resource_access_snapshot(&target)
                .await,
            Err(ResourceAccessStoreError::Corrupt)
        );
    }
    client.database(&name).drop().await.unwrap();
}

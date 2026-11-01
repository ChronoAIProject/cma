use super::*;
use crate::store::agent_provenance;
use layer2_core::sandbox::store::{AgentProvenanceSnapshot, AgentProvenanceStore};

type Trace = Arc<Mutex<Vec<(String, Document)>>>;

async fn observed_fixture(prefix: &str) -> (Client, String, MongoSandboxStore, AgentId, Trace) {
    let (admin, name) = database(prefix).await;
    let trace = Trace::default();
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
    let ResourceTarget::Agent { agent_id } = target else {
        unreachable!()
    };
    (admin, name, store, agent_id, trace)
}

fn assert_snapshot_commands(trace: &Trace, finds: usize) {
    let commands = trace.lock().unwrap().clone();
    assert_eq!(
        commands.len(),
        finds + 2,
        "one aggregate, optional fallback, abort: {commands:?}"
    );
    assert_eq!(commands[0].0, "aggregate");
    let first = &commands[0].1;
    assert!(command_uses_snapshot_read(first));
    assert!(first.get_bool("startTransaction").unwrap());
    for (index, (name, command)) in commands.iter().enumerate() {
        assert_eq!(
            name,
            if index == 0 {
                "aggregate"
            } else if index == finds + 1 {
                "abortTransaction"
            } else {
                "find"
            }
        );
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

async fn baseline(store: &MongoSandboxStore, id: &AgentId) -> AgentProvenanceSnapshot {
    AgentProvenanceSnapshot {
        resource: store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: id.clone(),
            })
            .await
            .unwrap(),
        creation: store
            .agent_creation_provenance
            .find_one(doc! {"_id": id.as_str()})
            .await
            .unwrap()
            .as_ref()
            .map(agent_provenance::read)
            .transpose()
            .unwrap(),
        first_log: store.agent_log_entry(id, 1).await.unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_batch_uses_one_read_only_snapshot_and_preserves_creation_evidence() {
    let (admin, name, store, id, trace) = observed_fixture("provenance_batch").await;
    let legacy = baseline(&store, &id).await;
    let inferred = legacy.creation_provenance(&id).unwrap();
    for creation in [None, Some(inferred)] {
        if let Some(creation) = &creation {
            store
                .agent_creation_provenance
                .insert_one(agent_provenance::document(creation).unwrap())
                .await
                .unwrap();
        }
        let expected = AgentProvenanceSnapshot {
            creation,
            ..legacy.clone()
        };
        trace.lock().unwrap().clear();
        assert_eq!(
            store.agent_provenance_snapshot(&id).await.unwrap(),
            expected
        );
        assert_snapshot_commands(&trace, 0);
        assert_eq!(baseline(&store, &id).await, expected);
    }
    // Historical absence remains optional; callers retain their existing
    // evidence-based fallback instead of inventing a new creation record.
    store
        .agent_creation_provenance
        .delete_many(doc! {})
        .await
        .unwrap();
    store
        .agent_log
        .delete_one(doc! {"agent_id": id.as_str(), "seq": 1_i64})
        .await
        .unwrap();
    let snapshot = store.agent_provenance_snapshot(&id).await.unwrap();
    assert!(snapshot.creation.is_none());
    assert!(snapshot.first_log.is_none());
    assert_eq!(snapshot, baseline(&store, &id).await);
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_batch_keeps_transaction_snapshot_and_observes_new_revocation() {
    let (admin, name, store, id, _) = observed_fixture("provenance_revocation").await;
    let before = baseline(&store, &id).await;
    let created = store
        .mutate_share(mutation(&before.resource, "batch-create", ShareMode::View))
        .await
        .unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let retained = agent_provenance::read_snapshot(&store, &id, &mut session)
        .await
        .ok()
        .unwrap();
    assert_eq!(
        retained.resource.shares,
        std::slice::from_ref(&created.share)
    );
    let mut revoke = mutation(&retained.resource, "batch-revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: created.share.id,
    };
    revoke.expected_share_revision = Some(1);
    store.mutate_share(revoke).await.unwrap();
    assert_eq!(
        agent_provenance::read_snapshot(&store, &id, &mut session)
            .await
            .ok()
            .unwrap(),
        retained
    );
    session.abort_transaction().await.unwrap();
    let current = store.agent_provenance_snapshot(&id).await.unwrap();
    assert!(current.resource.shares.is_empty());
    assert_eq!(current.resource.policy.unwrap().revision, 2);
    assert_eq!(current.resource.lineage, retained.resource.lineage);
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_batch_rejects_corrupt_ancestry_indexes_policy_and_evidence() {
    let (admin, name, store, id, _) = observed_fixture("provenance_corrupt").await;
    let before = baseline(&store, &id).await;
    let created = store
        .mutate_share(mutation(&before.resource, "batch-corrupt", ShareMode::View))
        .await
        .unwrap();
    let creation = before.creation_provenance(&id).unwrap();
    store
        .agent_creation_provenance
        .insert_one(agent_provenance::document(&creation).unwrap())
        .await
        .unwrap();
    let lineage = &before.resource.lineage;
    let root = lineage.sandbox.id.as_str();
    let tree = lineage.worktree.as_ref().unwrap().id.as_str();
    let workspace = lineage.workspace.as_ref().unwrap().id.as_str();
    let first_log = store
        .agent_log
        .find_one(doc! {"agent_id": id.as_str(), "seq": 1_i64})
        .await
        .unwrap()
        .unwrap();
    for (collection, key, fields) in [
        (&store.agents, id.as_str(), doc! {"worktree_id": "wrong"}),
        (
            &store.agents,
            id.as_str(),
            doc! {"record.worktree_id": [tree]},
        ),
        (
            &store.agents,
            id.as_str(),
            doc! {"record.worktree_id": "missing", "worktree_id": "missing"},
        ),
        (
            &store.agents,
            id.as_str(),
            doc! {"record.owner": "foreign", "owner": "foreign"},
        ),
        (
            &store.worktrees,
            tree,
            doc! {"record.workspace_id": "missing", "workspace_id": "missing"},
        ),
        (&store.worktrees, tree, doc! {"record.agent_id": "foreign"}),
        (&store.worktrees, tree, doc! {"workspace_id": [workspace]}),
        (
            &store.workspaces,
            workspace,
            doc! {"record.sandbox_id": "missing", "sandbox_id": "missing"},
        ),
        (&store.workspaces, workspace, doc! {"owner": "foreign"}),
        (
            &store.sandboxes,
            root,
            doc! {"record.owner": "foreign", "owner": "foreign"},
        ),
        (
            &store.resource_policies,
            root,
            doc! {"owner_kind": "organization"},
        ),
        (
            &store.resource_policies,
            root,
            doc! {"record.sandbox_created_at": MINUTE},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"revision": 2_i64},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"sandbox_id": [root]},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"selector_id": [id.as_str()]},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"selector_kind": ["agent"]},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"revoked": [false]},
        ),
        (
            &store.resource_shares,
            created.share.id.as_str(),
            doc! {"selector_kind": "sandbox", "selector_id": [root]},
        ),
        (
            &store.agent_creation_provenance,
            id.as_str(),
            doc! {"record.agent_id": "foreign"},
        ),
        (
            &store.agent_log,
            first_log.get_str("_id").unwrap(),
            doc! {"record.agent_id": "foreign"},
        ),
    ] {
        let filter = doc! {"_id": key};
        let original = collection.find_one(filter.clone()).await.unwrap().unwrap();
        collection
            .update_one(filter.clone(), doc! {"$set": fields.clone()})
            .await
            .unwrap();
        assert!(
            matches!(
                store.agent_provenance_snapshot(&id).await,
                Err(SandboxStoreError::Corrupt(_))
            ),
            "accepted malformed {}: {fields:?}",
            collection.name()
        );
        collection.replace_one(filter, original).await.unwrap();
    }
    for (collection, key) in [
        (&store.worktrees, tree),
        (&store.workspaces, workspace),
        (&store.sandboxes, root),
    ] {
        let filter = doc! {"_id": key};
        let original = collection.find_one(filter.clone()).await.unwrap().unwrap();
        collection.delete_one(filter).await.unwrap();
        assert!(matches!(
            store.agent_provenance_snapshot(&id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        collection.insert_one(original).await.unwrap();
    }
    assert_eq!(
        store
            .agent_provenance_snapshot(&AgentId("missing".into()))
            .await,
        Err(SandboxStoreError::NotFound)
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_batch_bounds_applicable_shares_without_hiding_capacity_or_corruption() {
    let (admin, name, store, id, trace) = observed_fixture("provenance_shares").await;
    let before = baseline(&store, &id).await;
    let created = store
        .mutate_share(mutation(&before.resource, "batch-share", ShareMode::View))
        .await
        .unwrap();
    let original = store
        .resource_shares
        .find_one(doc! {"_id": &created.share.id})
        .await
        .unwrap()
        .unwrap();
    store.resource_shares.delete_many(doc! {}).await.unwrap();
    let mut documents = Vec::new();
    for index in 0..MAX_ACTIVE_RESOURCE_SHARES {
        let mut row = original.clone();
        let share_id = format!("shr_{index:03}");
        row.insert("_id", &share_id);
        row.get_document_mut("record")
            .unwrap()
            .insert("id", share_id);
        // Exercise the largest applicable page near the batching threshold.
        row.insert("retained_metadata", "p".repeat(60 * 1024));
        if index % 2 == 0 {
            row.insert("selector_kind", "sandbox");
            row.insert("selector_id", before.resource.lineage.sandbox.id.as_str());
            row.get_document_mut("record").unwrap().insert(
                "selector",
                doc! {
                    "kind": "sandbox", "sandbox_id": before.resource.lineage.sandbox.id.as_str(),
                },
            );
        }
        documents.push(row);
    }
    store.resource_shares.insert_many(documents).await.unwrap();
    // Nonmatching projected grants must be excluded before the bounded page.
    for (key, fields) in [
        ("aaa_other_agent", doc! {"selector_id": "other-agent"}),
        ("aaa_other_sandbox", doc! {"sandbox_id": "other-sandbox"}),
        ("aaa_revoked", doc! {"revoked": true}),
    ] {
        let mut row = original.clone();
        row.insert("_id", key);
        row.extend(fields);
        store.resource_shares.insert_one(row).await.unwrap();
    }
    let expected = baseline(&store, &id).await;
    assert_eq!(expected.resource.shares.len(), MAX_ACTIVE_RESOURCE_SHARES);
    trace.lock().unwrap().clear();
    assert_eq!(
        store.agent_provenance_snapshot(&id).await.unwrap(),
        expected
    );
    assert_snapshot_commands(&trace, 0);
    store
        .resource_shares
        .insert_one(original.clone())
        .await
        .unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(matches!(
        agent_provenance::read_snapshot(&store, &id, &mut session).await,
        Err(crate::store::resource_access::ReadError::Domain(
            ResourceAccessStoreError::Capacity
        ))
    ));
    session.abort_transaction().await.unwrap();
    store
        .resource_shares
        .update_one(
            doc! {"_id": &created.share.id},
            doc! {"$set": {"revision": 99_i64}},
        )
        .await
        .unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(matches!(
        agent_provenance::read_snapshot(&store, &id, &mut session).await,
        Err(crate::store::resource_access::ReadError::Domain(
            ResourceAccessStoreError::Corrupt
        ))
    ));
    session.abort_transaction().await.unwrap();
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_batch_large_retained_records_fall_back_in_the_same_transaction() {
    let (admin, name, store, id, trace) = observed_fixture("provenance_large").await;
    let before = baseline(&store, &id).await;
    // Valid individual BSON documents whose combined result would exceed
    // 16 MiB must remain readable. Extra stored metadata is historically valid.
    for (collection, key) in [
        (
            &store.sandboxes,
            before.resource.lineage.sandbox.id.as_str(),
        ),
        (&store.agents, id.as_str()),
    ] {
        collection
            .update_one(
                doc! {"_id": key},
                doc! {"$set": {"retained_metadata": "p".repeat(9 * 1024 * 1024)}},
            )
            .await
            .unwrap();
        trace.lock().unwrap().clear();
        assert_eq!(store.agent_provenance_snapshot(&id).await.unwrap(), before);
        assert_snapshot_commands(&trace, 8);
    }
    // Returning to ordinary records must also return to one fresh query.
    for collection in [&store.agents, &store.sandboxes] {
        collection
            .update_many(doc! {}, doc! {"$unset": {"retained_metadata": ""}})
            .await
            .unwrap();
    }
    trace.lock().unwrap().clear();
    assert_eq!(store.agent_provenance_snapshot(&id).await.unwrap(), before);
    assert_snapshot_commands(&trace, 0);
    admin.database(&name).drop().await.unwrap();
}

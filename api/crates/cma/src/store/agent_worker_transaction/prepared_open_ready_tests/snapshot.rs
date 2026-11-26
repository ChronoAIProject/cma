use super::*;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_preserves_an_independent_parent_renewal() {
    use layer2_core::sandbox::store::SandboxWorkerStore;
    let (client, name, store, agent, mut observed) = fixture().await;
    let now = layer2_core::now_rfc3339();
    let until = crate::store::sandbox_mongo_integration_tests::work_lease_deadline();
    observed.sandbox = store
        .claim_sandbox_work(&observed.sandbox.id, "parent-worker", now.clone(), until)
        .await
        .unwrap()
        .unwrap();
    let independent = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let successor = MongoSandboxStore::open(&independent, &name).await.unwrap();
    let requested =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(120));
    let extended = successor
        .renew_sandbox_work(&observed.sandbox, &now, &requested)
        .await
        .unwrap()
        .unwrap();
    assert!(extended > observed.sandbox.work.lease.as_ref().unwrap().expires_at);
    assert!(store
        .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed.clone())
        .await
        .unwrap());
    let parent = successor
        .get_sandbox(&observed.sandbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.work.lease.as_ref().unwrap().expires_at, extended);
    assert_eq!(parent.revision, observed.sandbox.revision);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_reads_eligibility_once_inside_its_publication_transaction() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let captured = trace.clone();
    let name = format!("cma_prepared_snapshot_{}", uuid::Uuid::new_v4().simple());
    let database = name.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.db == database && event.command_name == "find" {
                let command = event.command;
                captured.lock().unwrap().push((
                    command.get_str("find").unwrap().to_owned(),
                    command.get_document("lsid").ok().cloned(),
                    command.get_i64("txnNumber").ok(),
                    command.get_bool("autocommit").ok(),
                ));
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, sandbox, workspace, worktree) = hierarchy(&store).await;
    let observed = PreparedAgentOpenContext {
        sandbox,
        workspace,
        worktree,
    };
    trace.lock().unwrap().clear();
    assert!(store
        .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed,)
        .await
        .unwrap());
    let commands = trace.lock().unwrap().clone();
    let relevant: Vec<_> = commands
        .iter()
        .filter(|(collection, ..)| {
            collection.as_str() == store.agents.name()
                || collection.as_str() == store.native_prepared.name()
        })
        .collect();
    // Eligibility consumes the first Agent/preparation pair once. The shared
    // complete-log audit then rereads the Agent to check integrity/quarantine;
    // that read must remain in the very same publication snapshot as well.
    assert_eq!(relevant.len(), 3);
    assert_eq!(relevant[0].0, store.agents.name());
    assert_eq!(relevant[1].0, store.native_prepared.name());
    assert_eq!(relevant[2].0, store.agents.name());
    let (_, session, transaction, autocommit) = relevant[0];
    assert!(session.is_some() && transaction.is_some());
    assert_eq!(*autocommit, Some(false));
    for (_, read_session, read_transaction, read_autocommit) in &commands {
        assert_eq!(read_session, session);
        assert_eq!(read_transaction, transaction);
        assert_eq!(*read_autocommit, Some(false));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_ineligible_snapshot_leaves_all_publication_records_unchanged() {
    let (client, name, store, agent, observed) = fixture().await;
    let mut incompatible = observed.clone();
    incompatible.sandbox.credential.delivered = false;
    let collections = [
        &store.agents,
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.native_prepared,
        &store.agent_log,
        &store.sandbox_events,
    ];
    let mut before = Vec::new();
    for collection in collections {
        use futures_util::TryStreamExt;
        before.push(
            collection
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .await
                .unwrap()
                .try_collect::<Vec<Document>>()
                .await
                .unwrap(),
        );
    }
    assert!(!store
        .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), incompatible,)
        .await
        .unwrap());
    for (collection, expected) in collections.into_iter().zip(before) {
        use futures_util::TryStreamExt;
        let current = collection
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .await
            .unwrap()
            .try_collect::<Vec<Document>>()
            .await
            .unwrap();
        assert_eq!(
            current,
            expected,
            "{} changed during ineligible publication",
            collection.name()
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_preparation_write_conflicts_with_the_consumed_snapshot() {
    let (client, name, store, agent, observed) = fixture().await;
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let revision = prepared_open_ready::proof_revision(&store, &agent, &observed, &mut session)
        .await
        .unwrap_or_else(|_| panic!("eligible preparation"));
    store
        .native_prepared
        .update_one(
            doc! {"_id": observed.sandbox.id.as_str()},
            doc! {"$inc": {"revision": 1_i64, "record.payload.revision": 1_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        prepared_open_ready::fence(&store, &agent, &observed.worktree, &observed,
            revision, &mut session).await,
        Err(ApplyError::Driver(error)) if is_transient(&error)
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_refuses_a_worktree_schema_changed_after_snapshot() {
    let (client, name, store, agent, observed) = fixture().await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let worktrees = peer
        .database(&name)
        .collection::<Document>(store.worktrees.name());
    let tree_id = observed.worktree.id.as_str();
    let future_schema = DOCUMENT_SCHEMA_VERSION + 1;
    let done = completion(&agent, &observed.worktree);
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let revision = prepared_open_ready::proof_revision(&store, &agent, &observed, &mut session)
        .await
        .unwrap_or_else(|_| panic!("eligible preparation"));

    assert_eq!(
        worktrees
            .update_one(
                doc! {"_id": tree_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION},
                doc! {"$set": {"document_schema_version": future_schema}},
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
    assert!(matches!(
        prepared_open_ready::fence(&store, &agent, &observed.worktree, &observed,
            revision, &mut session).await,
        Err(ApplyError::Driver(error)) if is_transient(&error)
    ));
    session.abort_transaction().await.unwrap();
    assert!(matches!(
        store
            .commit_prepared_agent_open_ready(done, observed.clone())
            .await,
        Err(SandboxStoreError::Corrupt(message))
            if message.starts_with("unsupported document schema version ")
    ));
    let persisted = worktrees
        .find_one(doc! {"_id": tree_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted.get_i64("document_schema_version").unwrap(),
        future_schema
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    for (collection, id) in [
        (&store.sandboxes, observed.sandbox.id.as_str()),
        (&store.workspaces, observed.workspace.id.as_str()),
        (&store.worktrees, tree_id),
        (&store.native_prepared, observed.sandbox.id.as_str()),
    ] {
        let row = collection
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("agent_prepared_open_ready_fence"));
    }
    client.database(&name).drop().await.unwrap();
}

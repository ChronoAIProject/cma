use super::*;
use crate::store::native_prepared::tests::agent_replay_tests::{completion, hierarchy, intent};
use layer2_core::sandbox::store::{AgentStore, AgentWorkerStore, SandboxStore};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_pair_parent_write_conflicts_with_lifecycle_after_snapshot_read() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_prepared_pair_race_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, mut parent, workspace, tree) = hierarchy(&store).await;
    let pending = intent(&agent, &parent, &workspace, &tree);
    store.commit_agent_work(pending.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    // Anchor the old Mongo snapshot before lifecycle admission commits outside
    // this transaction. The real publication must conflict on its parent write.
    scope
        .find_one(&store.sandboxes, doc! {"_id": parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let revision = parent.revision;
    parent
        .begin_pause("2026-09-05T00:00:00Z".into(), None)
        .unwrap();
    store.save_sandbox(revision, parent).await.unwrap();
    let completed = completion(&pending.agent);
    assert!(matches!(apply_publication(&store, &completed, scope).await,
        Err(ApplyError::Driver(error)) if is_transient(&error)));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap(),
        pending.agent
    );
    assert!(matches!(
        store.commit_agent_work(completed).await,
        Err(SandboxStoreError::Conflict)
    ));
    client.database(&name).drop().await.unwrap();
}

//! Exercise production store paths; byte expectations serialize fixtures only.
use super::*;
use crate::store::mongo_read_telemetry::{isolated_test, snapshot};
use layer2_core::sandbox::{
    store::{AgentReadQueryStore, SandboxLifecycleExecutionStore},
    AgentListQuery, SandboxAdminStore,
};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn measured_agent_inventories_keep_results_and_count_valid_and_corrupt_rows() {
    let (client, name) = database("read_agent_telemetry").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_read_telemetry");
    store.create_sandbox(parent.clone()).await.unwrap();
    let (_, agent) = worktree_and_agent(&parent.id);
    store.create_agent(agent.clone()).await.unwrap();
    let bytes = crate::store::agents::agent_document(&agent)
        .unwrap()
        .to_vec()
        .unwrap()
        .len() as u64;
    isolated_test(async {
        let page = PageRequest::default();
        assert_eq!(
            store.list_agents(&agent.owner, &page).await.unwrap().items,
            vec![agent.clone()]
        );
        assert_eq!(
            store
                .list_owned_agents(&agent.owner, &AgentListQuery::default(), &page)
                .await
                .unwrap()
                .items,
            vec![agent.clone()]
        );
        assert_eq!(
            store.sandbox_agents(&parent.id, &page).await.unwrap().items,
            vec![agent.clone()]
        );
        assert_eq!(
            store
                .read_admin_sandbox(&parent.id)
                .await
                .unwrap()
                .unwrap()
                .agents
                .len(),
            1
        );
        let mut session = client.start_session().await.unwrap();
        let loaded = crate::store::lifecycle_execution::load_sandbox_agents(
            &store,
            &parent.id,
            &mut session,
        )
        .await;
        assert!(loaded.is_ok());
        assert_eq!(loaded.ok().unwrap(), vec![agent.clone()]);
        assert_eq!(snapshot("agents"), [5, 0, 5, 5 * bytes]);
        // This projection must count its encoded size, not the whole Agent size.
        assert_eq!(
            store.due_agent_work(NOW, 10).await.unwrap(),
            vec![agent.id.clone()]
        );
        let projected = doc! {"_id":agent.id.as_str()}.to_vec().unwrap().len() as u64;
        assert_eq!(snapshot("agents"), [6, 0, 6, 5 * bytes + projected]);
        assert!(store
            .live_agent_connections(&page)
            .await
            .unwrap()
            .items
            .is_empty());
        assert_eq!(snapshot("agents"), [7, 0, 6, 5 * bytes + projected]);
        // A typed point lookup has no borrowed raw boundary here and is unmeasured.
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(snapshot("agents")[0], 7);
        store
            .agents
            .update_one(
                doc! {"_id":agent.id.as_str()},
                doc! {"$set":{"revision":99_i64}},
            )
            .await
            .unwrap();
        assert!(matches!(
            store.sandbox_agents(&parent.id, &page).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(snapshot("agents"), [8, 1, 7, 6 * bytes + projected]);
        let failed = crate::store::lifecycle_execution::load_sandbox_agents(
            &store,
            &parent.id,
            &mut session,
        )
        .await;
        assert!(matches!(
            failed,
            Err(
                crate::store::lifecycle_execution::LifecycleLoadError::Domain(
                    SandboxStoreError::Corrupt(_)
                )
            )
        ));
        assert_eq!(snapshot("agents"), [9, 2, 8, 7 * bytes + projected]);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn measured_event_pages_and_validation_keep_gaps_fail_closed() {
    let (client, name) = database("read_event_telemetry").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_event_read_telemetry");
    store.create_sandbox(parent.clone()).await.unwrap();
    let first = store
        .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    let second = store
        .append_sandbox_event(&parent.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    let bytes = event_document(&first).unwrap().to_vec().unwrap().len() as u64;
    assert_eq!(
        bytes,
        event_document(&second).unwrap().to_vec().unwrap().len() as u64
    );
    isolated_test(async {
        assert_eq!(
            store.sandbox_events(&parent.id, 0, 10).await.unwrap(),
            vec![first, second]
        );
        assert_eq!(snapshot("sandbox_events"), [1, 0, 2, 2 * bytes]);
        assert!(store
            .sandbox_events(&parent.id, 2, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(snapshot("sandbox_events")[0], 1); // no event query for an empty range
        let mut session = client.start_session().await.unwrap();
        assert!(crate::store::sandbox_events::validate_in_transaction(
            &store,
            &parent.id,
            2,
            &mut session
        )
        .await
        .is_ok());
        assert_eq!(snapshot("sandbox_events"), [2, 0, 4, 4 * bytes]);
        store
            .sandbox_events
            .delete_one(doc! {"sandbox_id":parent.id.as_str(), "seq":1_i64})
            .await
            .unwrap();
        assert_eq!(
            store.sandbox_events(&parent.id, 0, 10).await,
            Err(SandboxStoreError::SequenceConflict)
        );
        assert_eq!(snapshot("sandbox_events"), [3, 1, 5, 5 * bytes]);
        let failed = crate::store::sandbox_events::validate_in_transaction(
            &store,
            &parent.id,
            2,
            &mut session,
        )
        .await;
        assert!(matches!(
            failed,
            Err(crate::store::sandbox_events::TransactionEventError::Domain(
                SandboxStoreError::SequenceConflict
            ))
        ));
        assert_eq!(snapshot("sandbox_events"), [4, 2, 6, 6 * bytes]);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

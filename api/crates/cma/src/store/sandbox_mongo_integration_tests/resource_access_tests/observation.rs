use super::*;

pub(super) async fn cohort(
    store: &MongoSandboxStore,
    original: &ResourceAccessSnapshot,
) -> Vec<AgentId> {
    let mut ids = vec![original.lineage.agent.as_ref().unwrap().id.clone()];
    for index in 1..100 {
        let mut agent = original.lineage.agent.clone().unwrap();
        let mut worktree = original.lineage.worktree.clone().unwrap();
        agent.id = AgentId(format!("agt_observe_{index:03}"));
        agent.worktree_id = WorktreeId(format!("wkt_observe_{index:03}"));
        worktree.id = agent.worktree_id.clone();
        agent.creation_key = format!("observation-{index}");
        worktree.name = format!("wt-{}", index + 1);
        worktree.path = format!(
            "/home/cma/{}/.worktrees/{}",
            original.lineage.workspace.as_ref().unwrap().name,
            worktree.name
        );
        worktree.branch = format!("cma/{}", worktree.name);
        worktree.agent_id = Some(agent.id.clone());
        store
            .worktrees
            .insert_one(crate::store::worktrees::worktree_document(&worktree).unwrap())
            .await
            .unwrap();
        store
            .agents
            .insert_one(crate::store::agents::agent_document(&agent).unwrap())
            .await
            .unwrap();
        ids.push(agent.id);
    }
    ids
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_cohort_matches_individual_facts_and_isolates_absent_and_corrupt_targets() {
    let (client, name) = database("observation_facts").await;
    let (store, target) = fixture(&client, &name).await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    let mut ids = cohort(&store, &original).await;
    store
        .mutate_share(mutation(&original, "observation", ShareMode::View))
        .await
        .unwrap();
    let rows = store.agent_observation_snapshots(&ids).await.unwrap();
    for (id, row) in ids.iter().zip(rows) {
        assert_eq!(
            row.unwrap(),
            store
                .resource_access_snapshot(&ResourceTarget::Agent {
                    agent_id: id.clone()
                })
                .await
                .unwrap()
        );
    }
    ids[1] = AgentId("agt_missing".into());
    store
        .agents
        .update_one(
            doc! {"_id": ids[2].as_str()},
            doc! {"$set": {"record.worktree_id": "wkt_missing"}},
        )
        .await
        .unwrap();
    let rows = store.agent_observation_snapshots(&ids).await.unwrap();
    assert_eq!(rows[1], Err(ResourceAccessStoreError::NotFound));
    assert_eq!(rows[2], Err(ResourceAccessStoreError::Corrupt));
    assert!(rows[0].as_ref().unwrap().shares.len() == 1);
    assert!(rows[3..].iter().all(|row| row
        .as_ref()
        .is_ok_and(|snapshot| snapshot.shares.is_empty())));
    assert!(store
        .agent_observation_snapshots(&[])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .agent_observation_snapshots(&vec![ids[0].clone(); 101])
            .await,
        Err(ResourceAccessStoreError::Capacity)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_cohort_uses_one_snapshot_and_two_commands_for_one_hundred_agents() {
    let (admin, name) = database("observation_batch_commands").await;
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
    let ids = cohort(&store, &original).await;
    trace.lock().unwrap().clear();
    let rows = store.agent_observation_snapshots(&ids).await.unwrap();
    assert_eq!(rows.len(), 100);
    assert!(rows.iter().all(Result::is_ok));
    let commands = trace.lock().unwrap().clone();
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert_eq!(commands[0].0, "aggregate");
    assert_eq!(commands[1].0, "find");
    assert!(command_uses_snapshot_read(&commands[0].1));
    assert_eq!(
        commands[0].1.get_document("lsid").unwrap(),
        commands[1].1.get_document("lsid").unwrap()
    );
    assert!(commands[1]
        .1
        .get_document("readConcern")
        .unwrap()
        .contains_key("atClusterTime"));
    admin.database(&name).drop().await.unwrap();
}

use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentOperationIntent, AgentOperationRecord};
use tests::{key, persisted, publish, request};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_retained_transport_requires_durable_idle_intent_and_keeps_original_capability() {
    let old_origin = "https://old.example.test";
    let new_origin = "https://new.example.test";
    let (client, database, store, state) = persisted().await;
    let old = MongoAgentAccessToolSource::new(store.clone(), old_origin, &key(9)).unwrap();
    let source = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        new_origin,
        &[old_origin.into()],
        &key(9),
    )
    .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = old.capability(&scope).ok().unwrap();
    let open = request(&state);
    let initial = old.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    old.confirm_open(&initial, "original-native").await.unwrap();
    publish(&store, &scope.creation.agent_id).await;
    let mut agent = store
        .get_agent(&scope.creation.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.work.next_fence = 2;
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let retained = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert!(source
        .prepare_transport(&retained, new_origin, "original-native", 1)
        .await
        .is_err());
    let mut settings = agent.settings();
    settings.model = "model-a".into();
    agent.conversation.settings = Some(settings.clone());
    // Seed the selected setting in this disposable fixture. The ordinary worker
    // save deliberately cannot change settings owned by the operation ledger.
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"record.conversation.settings": record_bson(&agent.conversation.settings).unwrap()}},
        )
        .await
        .unwrap();
    agent.conversation.operation = Some(
        AgentOperationRecord::new(
            1,
            1,
            1,
            AgentOperationIntent::RefreshContext {
                service_origin: Some(new_origin.into()),
                revision: 2,
                readiness_receipt: "a".repeat(64),
                guidance: "Managed guidance".into(),
                previous_guidance: vec![],
                settings,
                working_directory: None,
            },
            fixtures::NOW.into(),
        )
        .unwrap(),
    );
    agent.conversation.last_seq = 1;
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    assert!(source
        .prepare_transport(&retained, old_origin, "original-native", 1)
        .await
        .is_err());
    assert!(source
        .prepare_transport(&retained, new_origin, "another-native", 1)
        .await
        .is_err());
    assert!(source
        .prepare_transport(&retained, new_origin, "original-native", 2)
        .await
        .is_err());
    let migrated = source
        .prepare_transport(&retained, new_origin, "original-native", 1)
        .await
        .unwrap();
    assert_eq!(
        migrated,
        HostedAccessToolBinding::new(
            new_origin,
            bearer.clone().into(),
            runtime.clone(),
            &open,
            2,
            Some("original-native".into())
        )
        .unwrap()
    );
    // Preparation cannot claim installation. A new reader still gets the old URL.
    assert_eq!(
        source.resolve(&runtime, &open, 2).await.unwrap().unwrap(),
        retained
    );
    assert!(source
        .confirm_transport(&retained, "original-native", 1)
        .await
        .is_err());
    assert!(source
        .confirm_transport(&migrated, "original-native", 2)
        .await
        .is_err());
    let identity = source.authenticate(&bearer).await.unwrap();
    source
        .confirm_transport(&migrated, "original-native", 1)
        .await
        .unwrap();
    source
        .confirm_transport(&migrated, "original-native", 1)
        .await
        .unwrap();
    assert_eq!(source.authenticate(&bearer).await.unwrap(), identity);
    assert_eq!(
        source.resolve(&runtime, &open, 2).await.unwrap().unwrap(),
        migrated
    );
    source
        .confirm_open(&migrated, "original-native")
        .await
        .unwrap();
    // Compatibility rollback changes public advertisement, not installed transport.
    let rollback = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        old_origin,
        &[new_origin.into()],
        &key(9),
    )
    .unwrap();
    assert_eq!(
        rollback.resolve(&runtime, &open, 2).await.unwrap().unwrap(),
        migrated
    );
    assert_eq!(rollback.authenticate(&bearer).await.unwrap(), identity);
    assert_eq!(
        store
            .agent_access_tools
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let saved = store
        .get_agent(&scope.creation.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(saved.profile, agent.profile);
    client.database(&database).drop().await.unwrap();
}

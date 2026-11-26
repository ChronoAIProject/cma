use super::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::{AgentOperationIntent, AgentOperationRecord};
use tests::{key, persisted, publish, request};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_retained_transport_uses_completed_connection_lease() {
    use layer2_core::sandbox::AgentWorkStep;
    let (client, database, store, state) = persisted().await;
    let old_origin = "https://old.example.test";
    let target = "https://new.example.test";
    let old = MongoAgentAccessToolSource::new(store.clone(), old_origin, &key(9)).unwrap();
    let source = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        target,
        &[old_origin.into()],
        &key(9),
    )
    .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let open = request(&state);
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = old.capability(&scope).ok().unwrap();
    let first = old.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    old.confirm_open(&first, "original-native").await.unwrap();
    publish(&store, &scope.creation.agent_id).await;
    let mut agent = store
        .get_agent(&scope.creation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let now = layer2_core::now_rfc3339();
    let later =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(300));
    agent
        .work
        .schedule(AgentWorkStep::PublishConnection, now.clone());
    let fence = agent
        .work
        .claim("retained-worker".into(), &now, later)
        .unwrap();
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let retained = source
        .resolve(&runtime, &open, fence)
        .await
        .unwrap()
        .unwrap();
    source
        .confirm_open(&retained, "original-native")
        .await
        .unwrap();
    // Publishing the attachment releases its lease while next_fence has already
    // advanced. Maintenance must use its own durable operation authority.
    agent
        .work
        .advance("retained-worker", fence, AgentWorkStep::None, now.clone())
        .unwrap();
    assert_eq!(agent.work.next_fence, fence + 1);
    assert!(agent.work.lease.is_none());
    let mut settings = agent.settings();
    settings.model = "model-a".into();
    agent.conversation.settings = Some(settings.clone());
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {
                "record.conversation.settings": record_bson(&agent.conversation.settings).unwrap()
            }},
        )
        .await
        .unwrap();
    agent.conversation.operation = Some(
        AgentOperationRecord::new(
            1,
            1,
            1,
            AgentOperationIntent::RefreshContext {
                service_origin: Some(target.into()),
                revision: 2,
                readiness_receipt: "a".repeat(64),
                guidance: "Managed guidance".into(),
                previous_guidance: vec![],
                settings,
                working_directory: None,
            },
            now.clone(),
        )
        .unwrap(),
    );
    agent.conversation.last_seq = 1;
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    // Released authority remains invalid for opening or confirming a session.
    assert!(source.resolve(&runtime, &open, fence).await.is_err());
    assert!(source
        .confirm_open(&retained, "original-native")
        .await
        .is_err());
    let migrated = source
        .prepare_transport(&retained, target, "original-native", 1)
        .await
        .unwrap();
    let original_work = agent.work.clone();
    for newer in [false, true] {
        agent.work = original_work.clone();
        if newer {
            agent.work.next_fence += 1;
        } else {
            agent
                .work
                .schedule(AgentWorkStep::PublishConnection, now.clone());
        }
        agent.revision += 1;
        store
            .save_agent(agent.revision - 1, agent.clone())
            .await
            .unwrap();
        assert!(source
            .prepare_transport(&retained, target, "original-native", 1)
            .await
            .is_err());
        assert!(source
            .confirm_transport(&migrated, "original-native", 1)
            .await
            .is_err());
    }
    agent.work = original_work;
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    // The native reload can race a lineage writer. Confirmation must wait for
    // the bounded conflict to settle without claiming installation beforehand.
    let mut competing = store.client.start_session().await.unwrap();
    start_transaction(&mut competing).await.unwrap();
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$inc": {"resource_share_guard": 1_i64}},
        )
        .session(&mut competing)
        .await
        .unwrap();
    let (confirmation, ()) = tokio::join!(
        source.confirm_transport(&migrated, "original-native", 1),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let unconfirmed = store
                .agent_access_tools
                .find_one(doc! {"agent_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            assert!(!unconfirmed
                .get_document("record")
                .unwrap()
                .contains_key("transport_origin"));
            competing.abort_transaction().await.unwrap();
        }
    );
    confirmation.unwrap();
    assert_eq!(
        source.authenticate(&bearer).await.unwrap().conversation,
        "original-native"
    );
    let saved = store
        .agent_access_tools
        .find_one(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.get_str("_id").unwrap(), records::hash(&bearer));
    assert_eq!(
        saved
            .get_document("record")
            .unwrap()
            .get_str("transport_origin")
            .unwrap(),
        "https://new.example.test/"
    );
    client.database(&database).drop().await.unwrap();
}

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

#[path = "publication_retry_tests.rs"]
mod publication_retry;

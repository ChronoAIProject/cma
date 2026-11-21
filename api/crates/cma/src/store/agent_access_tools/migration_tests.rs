use super::*;
use layer2_core::sandbox::store::*;
use tests::{key, persisted, publish, request};

const OLD: &str = "https://old.example.test";
const NEW: &str = "https://new.example.test";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_domain_migration_preserves_live_credentials_and_conversation() {
    let (client, database, store, state) = persisted().await;
    let old = MongoAgentAccessToolSource::new(store.clone(), OLD, &key(9)).unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let bearer = old.capability(&scope).ok().unwrap();
    let open = request(&state);
    let binding = old.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    old.confirm_open(&binding, "original-native").await.unwrap();
    publish(&store, &scope.creation.agent_id).await;
    let original = old.authenticate(&bearer).await.unwrap();

    // Reproduce the incident: changing only the public origin invalidates the receipt.
    let unsafe_cutover = MongoAgentAccessToolSource::new(store.clone(), NEW, &key(9)).unwrap();
    assert!(unsafe_cutover.authenticate(&bearer).await.is_err());
    let migrated = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        NEW,
        &[OLD.into()],
        &key(9),
    )
    .unwrap();
    assert_eq!(migrated.authenticate(&bearer).await.unwrap(), original);
    migrated.authenticate_setup(&bearer).await.unwrap();

    let mut agent = store.get_agent(&original.agent_id).await.unwrap().unwrap();
    agent.work.step = layer2_core::sandbox::AgentWorkStep::PublishConnection;
    agent.work.lease = state
        .resource
        .lineage
        .agent
        .as_ref()
        .unwrap()
        .work
        .lease
        .clone();
    agent.work.lease.as_mut().unwrap().fence = 2;
    agent.work.next_fence = 3;
    agent.revision += 1;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
    assert!(migrated.resolve(&runtime, &open, 1).await.is_err());
    let retained = migrated.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    let expected = HostedAccessToolBinding::new(
        OLD,
        bearer.clone().into(),
        runtime.clone(),
        &open,
        2,
        Some("original-native".into()),
    )
    .unwrap();
    assert_eq!(
        retained, expected,
        "do not replace a live endpoint or credential"
    );
    migrated
        .confirm_open(&retained, "original-native")
        .await
        .unwrap();
    assert_eq!(migrated.authenticate(&bearer).await.unwrap(), original);
    assert_eq!(old.authenticate(&bearer).await.unwrap(), original);
    assert_eq!(
        store
            .agent_access_tools
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let altered_endpoint = HostedAccessToolBinding::new(
        NEW,
        bearer.clone().into(),
        runtime,
        &open,
        2,
        Some("original-native".into()),
    )
    .unwrap();
    assert!(migrated
        .confirm_open(&altered_endpoint, "original-native")
        .await
        .is_err());
    let foreign = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        NEW,
        &["https://unrelated.example.test".into()],
        &key(9),
    )
    .unwrap();
    assert!(foreign.authenticate(&bearer).await.is_err());
    let wrong_key = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        NEW,
        &[OLD.into()],
        &key(10),
    )
    .unwrap();
    assert!(wrong_key.authenticate(&bearer).await.is_err());

    let mut stopped = store.get_agent(&original.agent_id).await.unwrap().unwrap();
    stopped.requested_stop_reason = Some(layer2_core::sandbox::AgentStopReason::Owner);
    stopped.revision += 1;
    store
        .save_agent(stopped.revision - 1, stopped)
        .await
        .unwrap();
    assert!(migrated.authenticate(&bearer).await.is_err());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_agent_access_new_enrollment_uses_canonical_origin_and_rejects_ambiguous_receipts() {
    let (client, database, store, state) = persisted().await;
    let source = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        NEW,
        &[OLD.into()],
        &key(9),
    )
    .unwrap();
    let runtime = RuntimeHandle("access-runtime".into());
    let scope = validation::scope(&state, &runtime).ok().unwrap();
    let open = request(&state);
    let bearer = source.capability(&scope).ok().unwrap();
    let binding = source.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    assert_eq!(
        binding,
        HostedAccessToolBinding::new(NEW, bearer.clone().into(), runtime.clone(), &open, 1, None,)
            .unwrap()
    );
    source
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    publish(&store, &scope.creation.agent_id).await;
    source.authenticate(&bearer).await.unwrap();
    // Compatibility-first readers support both cohorts during rollout and rollback.
    let compat = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        OLD,
        &[NEW.into()],
        &key(9),
    )
    .unwrap();
    compat.authenticate(&bearer).await.unwrap();
    let mut document = store
        .agent_access_tools
        .find_one(doc! {"_id": records::hash(&bearer)})
        .await
        .unwrap()
        .unwrap();
    let other = source.capability_at(&scope, 1).ok().unwrap();
    document.insert("_id", records::hash(&other));
    store.agent_access_tools.insert_one(document).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(source.current_record(&scope, &mut session).await.is_err());
    session.abort_transaction().await.unwrap();
    client.database(&database).drop().await.unwrap();
}

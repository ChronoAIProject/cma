use super::*;
use layer2_core::sandbox::store::AgentProvenanceStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_refuses_tool_and_relay_authority_without_association_uniqueness() {
    let (client, name, store) = crate::store::bindings::mongo_tests::fixture().await;
    let (binding, conversation, request) = tests::setup(&store).await;
    let runtime = RuntimeHandle("provider-runtime".into());
    let snapshot = store
        .agent_provenance_snapshot(&conversation.agent_id)
        .await
        .unwrap();
    let (scope, _) =
        validation::validate(&snapshot, &binding, &conversation, &runtime, &request, 1).unwrap();
    assert!(MongoProviderToolSource::new(Arc::new(store))
        .resolve(&runtime, &request, 1)
        .await
        .unwrap()
        .is_some());
    let db = client.database(&name);
    db.collection::<Document>(CONVERSATIONS)
        .drop_index("binding_generation_agent")
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let before = reopened.bindings.records.find_one(doc! {}).await.unwrap();
    let source = MongoProviderToolSource::new(reopened.clone());
    let resolve = source.resolve(&runtime, &request, 1).await;
    let mut session = client.start_session().await.unwrap();
    let relay = source.current_relay_lineage(&scope, &mut session).await;
    let after = reopened.bindings.records.find_one(doc! {}).await.unwrap();
    let opens = reopened
        .bindings
        .tool_opens
        .count_documents(doc! {})
        .await
        .unwrap();
    db.drop().await.unwrap();
    assert!(matches!(resolve, Err(AdapterError::Unavailable(_))));
    assert!(matches!(
        relay,
        Err(transaction::Error::Domain(AdapterError::Unavailable(_)))
    ));
    assert_eq!(before, after);
    assert_eq!(opens, 0);
}

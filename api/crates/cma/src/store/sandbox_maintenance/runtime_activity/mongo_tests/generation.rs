use super::*;

async fn server_process(client: &Client) -> mongodb::bson::oid::ObjectId {
    client
        .database("admin")
        .run_command(doc! {"hello":1})
        .await
        .unwrap()
        .get_document("topologyVersion")
        .unwrap()
        .get_object_id("processId")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn collection_replacement_refuses_old_proof_even_with_identical_rows() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_generation").await;
    let first = observe(&store, &sandbox.id).await;
    let proof = fixture_proof(first.clone());
    let process = server_process(&client).await;
    let root = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let saved = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();

    // Use actual Mongo catalog identity and actual stored metadata. Replacing
    // the collection changes its UUID without changing server or aggregate IDs.
    store.agents.drop().await.unwrap();
    assert!(matches!(
        store
            .observe_runtime_activity(&sandbox.id, &context(&store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(_)
    ));
    store.agents.insert_one(saved.clone()).await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        saved
    );
    assert_eq!(server_process(&client).await, process);
    let replaced = observe(&store, &sandbox.id).await;
    assert_eq!(replaced.vector, first.vector);
    assert_eq!(replaced.desktop, first.desktop);
    assert_eq!(replaced.source, first.source);
    assert_ne!(replaced.storage_incarnation, first.storage_incarnation);
    assert!(proof.compare_current(&replaced, now_ms().unwrap()).is_err());

    let mut session = transaction(&store, &proof).await;
    assert!(validate_for_admission(
        &store,
        &root,
        std::slice::from_ref(&agent),
        &proof,
        now_ms().unwrap(),
        &mut session
    )
    .await
    .is_err());
    session.abort_transaction().await.unwrap();
    assert_eq!(proof.final_observation, first);

    // Only a new fixture interval over the new generation can validate. This
    // helper's synthetic elapsed time does not claim real production quietness.
    let fresh = fixture_proof(replaced);
    let mut session = transaction(&store, &fresh).await;
    validate_for_admission(
        &store,
        &root,
        &[agent],
        &fresh,
        now_ms().unwrap(),
        &mut session,
    )
    .await
    .unwrap();
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn publication_source_proof_is_bounded_and_exact_across_seals_after_later_appends() {
    let (client, name) = database("segment_receipts").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 100).await;
    for range in [0..1, 1..5, 198..202] {
        let mut session = client.start_session().snapshot(true).await.unwrap();
        assert!(crate::store::agent_log_receipts::matches(
            &store,
            &agent,
            &entries[range],
            &mut session
        )
        .await
        .unwrap());
    }
    let mut changed = entries[1..5].to_vec();
    changed[1].kind = AgentLogKind::ModeChanged {
        mode: "different".into(),
    };
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        crate::store::agent_log_receipts::matches(&store, &agent, &changed, &mut session).await,
        Err(SandboxStoreError::SequenceConflict)
    ));
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(!crate::store::agent_log_receipts::matches(
        &store,
        &agent,
        &[entries[1].clone(), entries[3].clone()],
        &mut session
    )
    .await
    .unwrap());
    store
        .agent_log
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":3_i64})
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(crate::store::agent_log_receipts::matches(
        &store,
        &agent,
        &entries[1..5],
        &mut session
    )
    .await
    .is_err());
    client.database(&name).drop().await.unwrap();
}

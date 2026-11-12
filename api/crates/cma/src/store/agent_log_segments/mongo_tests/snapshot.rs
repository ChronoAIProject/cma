use super::*;
use crate::store::agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn captured_head_seals_and_source_rows_keep_one_snapshot_during_changes() {
    let (client, name) = database("snapshot").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = PreparedIntegrity
        .read_head(&store, &agent.id, &mut session)
        .await
        .unwrap()
        .unwrap();
    // A second client changes both source and manifest after capture. Every
    // subsequent proof query must still see the original coherent snapshot.
    let (other, _) = database("unused").await;
    other
        .database(&name)
        .collection::<Document>("agent_log")
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":3_i64})
        .await
        .unwrap();
    other
        .database(&name)
        .collection::<Document>(SEGMENTS)
        .delete_one(doc! {"agent_id":agent.id.as_str(),"ordinal":1_i64})
        .await
        .unwrap();
    let mut seen = Vec::new();
    visit_range(
        &store,
        captured.segmented.as_ref().unwrap(),
        LogRange {
            after: 1,
            through: 8,
            limit: 7,
        },
        &mut session,
        true,
        |entry| {
            seen.push(entry);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(seen, entries[1..]);
    assert!(store.agent_log_range(&agent.id, 1, 8, 7).await.is_err());
    drop(session);
    client.database(&name).drop().await.unwrap();
}

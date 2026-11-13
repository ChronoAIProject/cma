use super::*;
use crate::store::agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity};
use layer2_core::sandbox::store::SandboxStoreError;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn every_segmented_page_refuses_source_before_sequence_one_without_repair() {
    let (client, name) = database("range_floor").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    for seq in [0_i64, -1, i64::MIN] {
        // Corruption may predate schema validation or arrive through restore.
        // Neither a sealed page nor an empty page may hide an invalid prefix.
        let mut row = log_document(&entries[1]).unwrap();
        row.insert("_id", format!("out-of-range:{seq}"));
        row.insert("seq", seq);
        row.get_document_mut("record").unwrap().insert("seq", seq);
        store
            .agent_log
            .insert_one(row.clone())
            .bypass_document_validation(true)
            .await
            .unwrap();
        for (after, through) in [(0, 2), (2, 5), (6, 8), (8, 8)] {
            assert!(matches!(
                store.agent_log_range(&agent.id, after, through, 20).await,
                Err(SandboxStoreError::SequenceConflict)
            ));
            let mut delivered = 0;
            assert!(matches!(
                crate::store::agent_log_store::visit_page(
                    &store,
                    &agent.id,
                    after,
                    through,
                    20,
                    true,
                    &mut |_| {
                        delivered += 1;
                        Ok(())
                    },
                )
                .await,
                Err(SandboxStoreError::SequenceConflict)
            ));
            assert_eq!(delivered, 0);
        }
        assert!(matches!(
            store.agent_log_entry(&agent.id, 2).await,
            Err(SandboxStoreError::SequenceConflict)
        ));
        let filter = doc! {"_id":row.get_str("_id").unwrap()};
        assert_eq!(
            store.agent_log.find_one(filter.clone()).await.unwrap(),
            Some(row)
        );
        assert_eq!(
            collection(&store, AUDITS)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        store.agent_log.delete_one(filter).await.unwrap();
        assert_eq!(
            store.agent_log_range(&agent.id, 0, 8, 20).await.unwrap(),
            entries
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn source_bound_probes_share_the_captured_page_snapshot() {
    let (client, name) = database("range_capture").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = PreparedIntegrity
        .read_head(&store, &agent.id, &mut session)
        .await
        .unwrap()
        .unwrap();
    let mut row = log_document(&entries[1]).unwrap();
    row.insert("_id", "later-invalid-prefix");
    row.insert("seq", 0_i64);
    store
        .agent_log
        .insert_one(row)
        .bypass_document_validation(true)
        .await
        .unwrap();
    let mut seen = Vec::new();
    visit_range(
        &store,
        captured.segmented.as_ref().unwrap(),
        LogRange {
            after: 1,
            through: 6,
            limit: 5,
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
    assert_eq!(seen, entries[1..6]);
    assert!(matches!(
        store.agent_log_range(&agent.id, 1, 6, 5).await,
        Err(SandboxStoreError::SequenceConflict)
    ));
    drop(session);
    db.drop().await.unwrap();
}

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_ranges_replay_original_rows_across_boundaries_and_frozen_heads() {
    let (client, name) = database("segment_range").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    for after in 0..=8 {
        for through in after..=8 {
            for limit in [1, 2, 4, 20, 100, 200] {
                let expected =
                    &entries[after as usize..(through.min(after + u64::from(limit))) as usize];
                assert_eq!(
                    store
                        .agent_log_range(&agent.id, after, through, limit)
                        .await
                        .unwrap(),
                    expected
                );
            }
        }
    }
    layer2_core::sandbox::store::agent_log_contract::assert_tail_page_contract(&store, &entries)
        .await;
    for (after, through, limit) in [(0, 0, 1), (0, 3, 2), (2, 5, 3), (8, 8, 1)] {
        assert_eq!(
            tail_page(&store, &agent.id, after, through, limit)
                .await
                .unwrap(),
            entries
                [after.saturating_sub(1) as usize..through.min(after + u64::from(limit)) as usize]
        );
    }
    for entry in &entries {
        assert_eq!(
            store.agent_log_entry(&agent.id, entry.seq).await.unwrap(),
            Some(entry.clone())
        );
    }
    assert!(store.agent_log_entry(&agent.id, 9).await.unwrap().is_none());
    assert!(store
        .agent_log(&agent.id, u64::MAX, 200)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.agent_log_prefix(&agent.id, 8).await.unwrap(), entries);
    // Fitting future append changes no captured cursor or immutable prefix.
    let next = AgentLogEntry {
        seq: 9,
        ..entries[7].clone()
    };
    let row = log_document(&next).unwrap();
    let bytes = bson::serialize_to_vec(&row).unwrap().len() as i64;
    store.agent_log.insert_one(row).await.unwrap();
    collection(&store, HEADS).update_one(doc! {"_id":agent.id.as_str()}, doc! {"$inc":{"revision":1_i64,"last_seq":1_i64,"active_count":1_i64,"active_bytes":bytes,"lifetime_count":1_i64,"lifetime_bytes":bytes,"projection_through":1_i64}}).await.unwrap();
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {"$inc":{"last_seq":1_i64,"record.conversation.last_seq":1_i64,"agent_log_head_revision":1_i64}}).await.unwrap();
    assert_eq!(
        store.agent_log_range(&agent.id, 3, 8, 4).await.unwrap(),
        entries[3..7]
    );
    assert_eq!(
        tail_page(&store, &agent.id, 3, 8, 4).await.unwrap(),
        entries[2..7]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segmented_ranges_refuse_corruption_missing_sidecars_and_uncommitted_tail() {
    let (client, name) = database("segment_corrupt").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    for seq in [1, 3, 6, 8] {
        let filter = doc! {"agent_id":agent.id.as_str(),"seq":seq};
        let old = store
            .agent_log
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        store.agent_log.delete_one(filter.clone()).await.unwrap();
        assert!(store
            .agent_log_range(&agent.id, (seq - 1) as u64, seq as u64, 1)
            .await
            .is_err());
        assert!(
            tail_page(&store, &agent.id, (seq - 1) as u64, seq as u64, 1)
                .await
                .is_err()
        );
        store.agent_log.insert_one(old.clone()).await.unwrap();
        store
            .agent_log
            .update_one(
                filter.clone(),
                doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
            )
            .await
            .unwrap();
        if seq <= 6 {
            assert!(store
                .agent_log_range(&agent.id, (seq - 1) as u64, seq as u64, 1)
                .await
                .is_err());
        }
        store.agent_log.replace_one(filter, old).await.unwrap();
    }
    let head = collection(&store, HEADS)
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    collection(&store, HEADS)
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 8, 8, 1).await.is_err());
    assert!(tail_page(&store, &agent.id, 8, 8, 1).await.is_err());
    collection(&store, HEADS).insert_one(head).await.unwrap();
    for ordinal in 0..3_i64 {
        let filter = doc! {"agent_id":agent.id.as_str(),"ordinal":ordinal};
        let seal = collection(&store, SEGMENTS)
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        collection(&store, SEGMENTS)
            .delete_one(filter)
            .await
            .unwrap();
        assert!(store.agent_log_range(&agent.id, 2, 4, 2).await.is_err());
        assert!(tail_page(&store, &agent.id, 2, 4, 2).await.is_err());
        collection(&store, SEGMENTS).insert_one(seal).await.unwrap();
    }
    let extra = AgentLogEntry {
        seq: 9,
        ..entries[7].clone()
    };
    store
        .agent_log
        .insert_one(log_document(&extra).unwrap())
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 0, 1, 1).await.is_err());
    assert!(store.agent_log_range(&agent.id, 8, 8, 1).await.is_err());
    assert!(tail_page(&store, &agent.id, 8, 8, 1).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn segment_schema_is_additive_restartable_and_refuses_drift() {
    let (client, name) = database("segment_schema").await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    schema::migrate(&db).await.unwrap();
    schema::migrate(&db).await.unwrap();
    let (agent, _) = segmented(&store, 1).await;
    MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let fields = agent_replacement_fields(&agent).unwrap();
    store
        .agents
        .update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    db.collection::<Document>(segments::SEGMENTS)
        .drop_index("agent_log_segment_start")
        .await
        .unwrap();
    assert!(MongoSandboxStore::open(&client, &name).await.is_err());
    schema::migrate(&db).await.unwrap();
    db.run_command(doc! {"collMod":segments::HEADS,"validationLevel":"off"})
        .await
        .unwrap();
    assert!(schema::migrate(&db).await.is_err());
    db.drop().await.unwrap();
}

use layer2_core::sandbox::store::agent_log_contract::tail_page;

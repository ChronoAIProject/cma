use super::*;
use sha2::{Digest, Sha256};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn indexed_output_traversal_uses_one_batch_per_page() {
    let (client, trace) = fixtures::measured_client().await;
    let name = format!("cma_response_batch_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = fixtures::source(&store, 2, true).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    let mut reference = None;
    let mut observed = Vec::new();
    for limit in [1, 3, 8] {
        let mut reader = store
            .agent_output_reader(&agent, Some(3))
            .await
            .unwrap()
            .unwrap();
        let count = reader.metadata().index.as_ref().unwrap().changes;
        assert!(count > 8);
        let mut changes = Vec::new();
        trace.reset();
        for from in (0..count).step_by(usize::from(limit)) {
            let batch = reader.changes(from, limit).await.unwrap();
            assert_eq!(batch.len() as u64, (count - from).min(u64::from(limit)));
            changes.extend(batch);
        }
        assert!(reader.changes(count, limit).await.unwrap().is_empty());
        let wire = trace.finish();
        assert_eq!(wire[3], 0, "the reader must issue no writes");
        let commands = trace.output_commands();
        let serialized = serde_json::to_vec(&changes).unwrap();
        assert_eq!(changes.len() as u64, count);
        eprintln!(
            "indexed_output limit={limit} changes={count} finds={} get_mores={} sha256={:x}",
            commands[0],
            commands[1],
            Sha256::digest(&serialized)
        );
        observed.push((limit, count, commands));
        if let Some(expected) = &reference {
            assert_eq!(expected, &serialized);
        } else {
            reference = Some(serialized);
        }
    }
    db.drop().await.unwrap();
    for (limit, count, commands) in observed {
        assert_eq!(
            commands,
            [count.div_ceil(u64::from(limit)), 0],
            "page limit {limit}"
        );
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn indexed_output_probe_refuses_excess_evidence() {
    let (client, name) = database("response_excess_probe").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = fixtures::source(&store, 2, true).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    let evidence = &store.segment_logs.evidence;
    let head = Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let first = evidence
        .find_one(doc! {"_id":segments::projection::identity(&head,"output_change","3:0")})
        .await
        .unwrap()
        .unwrap();
    // Keep the seek key and its partial predicate. A damaged uniqueness
    // invariant lets a second row enter the same bounded ordinal range.
    let seek = evidence
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .find(|model| {
            model
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some("agent_log_output_change_seek")
        })
        .unwrap();
    evidence
        .drop_index("agent_log_output_change_seek")
        .await
        .unwrap();
    let mut seek = seek;
    seek.options.as_mut().unwrap().unique = None;
    evidence.create_index(seek).await.unwrap();
    let mut extra = first;
    extra.insert("_id", "synthetic-extra-output");
    extra.insert("entity_key", "synthetic-extra-output");
    evidence.insert_one(extra).await.unwrap();

    let mut reader = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        reader.changes(0, 1).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    drop(reader);
    db.drop().await.unwrap();
}

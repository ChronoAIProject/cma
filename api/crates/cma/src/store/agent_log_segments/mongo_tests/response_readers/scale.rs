use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn retained_response_points_stay_bounded_above_both_history_limits() {
    for closed in [1, 10, 100, 1000] {
        let (client, trace) = fixtures::measured_client().await;
        let name = format!("cma_response_scale_{}", uuid::Uuid::new_v4().simple());
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, rows) = fixtures::source(&store, closed, false).await;
        fixtures::certify(&client, &db, &agent, closed).await;
        if closed == 1000 {
            assert!(rows.len() > 10_128);
            assert!(
                rows.iter()
                    .map(|r| bson::serialize_to_vec(&log_document(r).unwrap())
                        .unwrap()
                        .len())
                    .sum::<usize>()
                    > 4 * 1024 * 1024
            );
        }
        // No admission or legacy optimization switch is enabled in this fixture.
        db.run_command(doc! {"profile":2,"slowms":0}).await.unwrap();
        let mut latencies = Vec::new();
        let mut last = None;
        for _ in 0..5 {
            trace.reset();
            let started = std::time::Instant::now();
            let consumed = sample(&store, &agent, &rows).await;
            latencies.push(started.elapsed().as_micros());
            let wire = trace.finish();
            assert_eq!(&consumed[..3], &[3, 0, 13]);
            assert!(consumed[3] < 16_384);
            assert!(wire[0] < 60 && wire[1] < 60 && wire[2] < 256 * 1024);
            assert_eq!(
                wire[3], 0,
                "a response reader must never write or control a transaction"
            );
            last = Some((consumed, wire));
        }
        latencies.sort_unstable();
        let (consumed, wire) = last.unwrap();
        db.run_command(doc! {"profile":0}).await.unwrap();
        assert_eq!(&consumed[..3], &[3, 0, 13]); // active eight + turn start + four selected originals
        assert!(consumed[3] < 16_384);
        let profile = db
            .collection::<Document>("system.profile")
            .find(doc! {"ns":{"$regex":format!("^{}\\.",name)}})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let mut seek = false;
        let mut examined = 0_i64;
        for entry in &profile {
            assert!(!entry.get_bool("hasSortStage").unwrap_or(false));
            assert!(!entry
                .get_str("planSummary")
                .unwrap_or("")
                .contains("COLLSCAN"));
            if let Ok(command) = entry.get_document("command") {
                assert!(![
                    "insert",
                    "update",
                    "delete",
                    "commitTransaction",
                    "abortTransaction"
                ]
                .iter()
                .any(|k| command.contains_key(*k)));
                if command.get_str("find").ok() == Some(EVIDENCE)
                    && command.get_str("hint").ok() == Some("agent_log_output_change_seek")
                {
                    seek = true;
                }
            }
            examined += entry
                .get_i64("docsExamined")
                .or_else(|_| entry.get_i32("docsExamined").map(i64::from))
                .unwrap_or(0);
        }
        assert!(seek, "requested output must use the numeric seek index");
        assert!(profile.len() < 5 * 60 && examined < 5 * 80);
        eprintln!("response_bounds segments={closed} source_operations={} source_rows={} source_bytes={} checkouts={} commands={} reply_bytes={} docs_examined_total={examined} samples=5 p50_us={} p95_us={} max_us={}",consumed[0],consumed[2],consumed[3],wire[0],wire[1],wire[2],latencies[2],latencies[4],latencies[4]);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn long_turn_seeks_numeric_output_ordinals_and_pages_original_rows() {
    let (client, name) = database("response_seek").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, rows) = fixtures::source(&store, 10, true).await;
    fixtures::certify(&client, &db, &agent, 10).await;
    let mut reader = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    let page = reader.changes(120, 8).await.unwrap();
    assert_eq!(
        page.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
        (120..128).collect::<Vec<_>>()
    );
    let seqs = page.iter().map(|c| c.source_sequence).collect::<Vec<_>>();
    assert_eq!(
        reader.points(&seqs).await.unwrap(),
        seqs.iter()
            .map(|s| rows[*s as usize - 1].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(reader.source_page(16, 32).await.unwrap(), rows[16..48]);
    assert!(reader.changes(u64::MAX, 8).await.is_err());
    assert!(reader.source_page(1, 32).await.is_err());
    drop(reader);
    db.drop().await.unwrap();
}

async fn sample(store: &MongoSandboxStore, agent: &Agent, rows: &[AgentLogEntry]) -> [u64; 4] {
    crate::store::mongo_read_telemetry::isolated_test(async {
        let mut reader = store
            .agent_output_reader(agent, Some(3))
            .await
            .unwrap()
            .unwrap();
        let metadata = reader.metadata().clone();
        let facts = metadata.turn.unwrap();
        assert_eq!(facts.facts.model.as_deref(), Some("original-model"));
        assert_eq!(facts.usage_sequence, Some(agent.conversation.last_seq));
        let changes = reader.changes(0, 8).await.unwrap();
        assert_eq!(changes.len(), 1);
        let seqs = [
            3,
            changes[0].source_sequence,
            facts.terminal_sequence.unwrap(),
            facts.usage_sequence.unwrap(),
        ];
        assert_eq!(
            reader.points(&seqs).await.unwrap(),
            seqs.iter()
                .map(|s| rows[*s as usize - 1].clone())
                .collect::<Vec<_>>()
        );
        assert!(reader.changes(1, 8).await.unwrap().is_empty());
        crate::store::mongo_read_telemetry::snapshot("agent_log")
    })
    .await
}

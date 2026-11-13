//! Reproducible local benchmark; synthetic rows only, no production connection.
use super::*;
use std::time::Instant;

fn projection() -> Document {
    doc! {"_id":1,"document_schema_version":1,"recorded_at":1,"record.id":1,
    "record.at":1,"record.actor":1,"record.action":1,"record.target":1,"record.outcome":1}
}

async fn measure(db: &Database, filter: Document, projected: bool) -> serde_json::Value {
    let mut command = doc! {"find":ACTIVITY,"filter":filter,"sort":{"recorded_at":-1,"_id":-1},"limit":51_i64,"batchSize":52_i32,"maxTimeMS":5000_i32,"readConcern":{"level":"majority"}};
    if projected {
        command.insert("projection", projection());
    }
    let explain = db
        .run_command(doc! {"explain":command.clone(),"verbosity":"executionStats"})
        .await
        .unwrap();
    let stats = explain.get_document("executionStats").unwrap();
    let plan = explain
        .get_document("queryPlanner")
        .unwrap()
        .get_document("winningPlan")
        .unwrap();
    let mut samples = Vec::new();
    let mut bytes = 0;
    for _ in 0..21 {
        let start = Instant::now();
        let response = db.run_command(command.clone()).await.unwrap();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        let cursor = response.get_document("cursor").unwrap();
        assert_eq!(cursor.get_i64("id").unwrap(), 0);
        bytes = cursor
            .get_array("firstBatch")
            .unwrap()
            .iter()
            .map(|row| {
                mongodb::bson::serialize_to_vec(row.as_document().unwrap())
                    .unwrap()
                    .len()
            })
            .sum::<usize>();
    }
    samples.remove(0); // Separate the warm-up from the reported warm repeated observations.
    samples.sort_by(f64::total_cmp);
    let number = |field| {
        stats
            .get_i64(field)
            .or_else(|_| stats.get_i32(field).map(i64::from))
            .unwrap()
    };
    serde_json::json!({"n":samples.len(),"p50_ms":samples[9],"p95_ms":samples[18],"max_ms":samples[19],
        "keys_examined":number("totalKeysExamined"),"docs_examined":number("totalDocsExamined"),
        "returned":number("nReturned"),"bson_batch_bytes":bytes,"plan":plan,"samples_ms":samples})
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_large_collection_index_projection_before_after() {
    let (client, name) = database().await;
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    let db = client.database(&name);
    let collection = db.collection::<Document>(ACTIVITY);
    let now = DateTime::now().timestamp_millis();
    let count = 100_000;
    for start in (0..count).step_by(1000) {
        collection
            .insert_many(
                (start..start + 1000)
                    .map(|i| document(i, DateTime::from_millis(now - (count - i) as i64 / 1000))),
            )
            .await
            .unwrap();
    }
    let boundary = now - 50;
    let deep = doc! {"$or":[{"recorded_at":{"$lt":DateTime::from_millis(boundary)}},
    {"recorded_at":DateTime::from_millis(boundary),"_id":{"$lt":"act_000050000"}}]};
    let before_first = measure(&db, doc! {}, false).await;
    let before_deep = measure(&db, deep.clone(), false).await;
    // Explicit local fixture DDL, never MongoActivityStore::open or a reader side effect.
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"recorded_at":-1,"_id":-1})
                .options(
                    IndexOptions::builder()
                        .name("activity_recorded_at_id_desc".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let after_full = measure(&db, doc! {}, false).await;
    let after_summary = measure(&db, doc! {}, true).await;
    let after_deep = measure(&db, deep, true).await;
    for result in [&after_full, &after_summary, &after_deep] {
        assert!(result["keys_examined"].as_i64().unwrap() <= 55, "{result}");
        assert!(result["docs_examined"].as_i64().unwrap() <= 51, "{result}");
        let plan = result["plan"].to_string();
        assert!(plan.contains("activity_recorded_at_id_desc"), "{plan}");
        assert!(
            !plan.contains("\"stage\":\"SORT\"") && !plan.contains("COLLSCAN"),
            "{plan}"
        );
    }
    assert!(
        after_summary["bson_batch_bytes"].as_u64().unwrap()
            < after_full["bson_batch_bytes"].as_u64().unwrap()
    );
    let full = store.list(&ActivityQuery::default()).await.unwrap();
    let summary = store
        .list_summaries(&ActivityQuery::default())
        .await
        .unwrap();
    assert_eq!(
        summary.items,
        full.items
            .into_iter()
            .map(ActivitySummary::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(summary.next_cursor, full.next_cursor);
    println!(
        "ACTIVITY_BENCHMARK {}",
        serde_json::json!({"rows":count,"before_first":before_first,"before_deep":before_deep,"after_index_full":after_full,"after_index_summary":after_summary,"after_deep_summary":after_deep})
    );
    db.drop().await.unwrap();
}

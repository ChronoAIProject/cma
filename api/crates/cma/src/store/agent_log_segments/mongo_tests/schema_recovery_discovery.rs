//! Index qualification only: discovery tuples never authorize recovery effects.
use super::recovery_shape_fixture::carrier;
use super::*;
use mongodb::bson::Bson;

const EARLIER: &str = "2026-11-10T00:00:00Z";
const DUE: &str = "2026-11-11T00:00:00Z";
const FUTURE: &str = "2026-11-12T00:00:00Z";
const PAGE: i64 = 32;

fn row(phase: &str, serial: usize, at: &str) -> Document {
    let mut row = carrier(phase);
    // Distinct synthetic index tuples test storage ordering, never receipt
    // authority. Actual codec-to-schema compatibility is qualified separately.
    row.insert("_id", format!("capacity-recovery:{serial:064x}"));
    let work = row.get_document_mut("work").unwrap();
    work.insert("ready_at", at);
    if phase == "attaching" {
        work.get_document_mut("lease")
            .unwrap()
            .insert("expires_at", at);
    }
    row
}

fn number(document: &Document, key: &str) -> i64 {
    match document.get(key).unwrap() {
        Bson::Int32(value) => i64::from(*value),
        Bson::Int64(value) => *value,
        other => panic!("unexpected count type: {other:?}"),
    }
}

fn assert_index_plan(value: &Bson, index: &str) -> bool {
    match value {
        Bson::Document(document) => {
            assert_ne!(document.get_str("stage").ok(), Some("COLLSCAN"));
            assert_ne!(document.get_str("stage").ok(), Some("SORT"));
            let here = document.get_str("indexName").ok() == Some(index);
            document.values().fold(here, |found, value| {
                assert_index_plan(value, index) || found
            })
        }
        Bson::Array(values) => values.iter().fold(false, |found, value| {
            assert_index_plan(value, index) || found
        }),
        _ => false,
    }
}

async fn page(db: &mongodb::Database, find: Document, index: &str) -> Vec<Document> {
    let explain = db
        .run_command(doc! {"explain":find.clone(),"verbosity":"executionStats"})
        .await
        .unwrap();
    let winning = explain
        .get_document("queryPlanner")
        .unwrap()
        .get("winningPlan")
        .unwrap();
    assert!(assert_index_plan(winning, index));
    let stats = explain.get_document("executionStats").unwrap();
    assert!(number(stats, "nReturned") <= PAGE);
    assert!(number(stats, "totalDocsExamined") <= PAGE);
    assert!(number(stats, "totalKeysExamined") <= 2 * (PAGE + 1));
    println!(
        "recovery index={index} returned={} examined_keys={} examined_documents={}",
        number(stats, "nReturned"),
        number(stats, "totalKeysExamined"),
        number(stats, "totalDocsExamined")
    );
    let result = db.run_command(find).await.unwrap();
    let cursor = result.get_document("cursor").unwrap();
    // Explicit single-batch pages leave no retained server cursor or hidden
    // getMore. The request retains the frozen limit and batch-size budgets.
    assert_eq!(cursor.get_i64("id").unwrap(), 0);
    let rows = cursor.get_array("firstBatch").unwrap();
    assert!(rows.len() <= PAGE as usize);
    assert!(
        rows.iter()
            .map(|row| bson::serialize_to_vec(row.as_document().unwrap())
                .unwrap()
                .len())
            .sum::<usize>()
            <= PAGE as usize * 512
    );
    rows.iter()
        .map(|v| v.as_document().unwrap().clone())
        .collect()
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn recovery_discovery_indexes_bound_each_keyset_page_and_exclude_terminal_work() {
    let (client, name) = database("recovery_seek").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let rows = db.collection::<Document>(RECOVERY_WORK);
    for count in [1_usize, 100, 1000] {
        rows.delete_many(doc! {}).await.unwrap();
        for start in (0..count).step_by(32) {
            let noise = (start..count.min(start + 32))
                .flat_map(|i| [row("completed", i, DUE), row("admitted", 1000 + i, FUTURE)]);
            rows.insert_many(noise).await.unwrap();
        }
        for (phase, field, index, offset) in [
            (
                "admitted",
                "work.ready_at",
                "agent_capacity_recovery_due",
                2000,
            ),
            (
                "attaching",
                "work.lease.expires_at",
                "agent_capacity_recovery_expired",
                3000,
            ),
        ] {
            rows.insert_many(
                (0..65).map(|i| row(phase, offset + i, if i < 33 { EARLIER } else { DUE })),
            )
            .await
            .unwrap();
            let mut cursor: Option<(String, String)> = None;
            let mut observed = Vec::new();
            loop {
                let mut filter =
                    doc! {"format_version":2_i64,"work.phase":phase,field:{"$lte":DUE}};
                if let Some((at, after)) = &cursor {
                    filter.insert(
                        "$or",
                        vec![doc! {field:{"$gt":at}}, doc! {field:at,"_id":{"$gt":after}}],
                    );
                }
                let found = page(
                    &db,
                    doc! {
                        "find":RECOVERY_WORK,"filter":filter,"sort":{field:1,"_id":1},
                        "projection":{"_id":1,field:1},"hint":index,"collation":{"locale":"simple"},
                        "limit":PAGE,"batchSize":PAGE,"singleBatch":true,"maxTimeMS":1000,
                    },
                    index,
                )
                .await;
                if found.is_empty() {
                    break;
                }
                for row in found {
                    let key = row.get_str("_id").unwrap().to_owned();
                    let work = row.get_document("work").unwrap();
                    let at = if phase == "admitted" {
                        work.get_str("ready_at").unwrap()
                    } else {
                        work.get_document("lease")
                            .unwrap()
                            .get_str("expires_at")
                            .unwrap()
                    };
                    let next = (at.to_owned(), key.clone());
                    assert!(cursor.as_ref().is_none_or(|previous| previous < &next));
                    cursor = Some(next);
                    observed.push(key);
                }
                assert!(observed.len() <= 65);
            }
            assert_eq!(
                observed,
                (0..65)
                    .map(|i| format!("capacity-recovery:{:064x}", offset + i))
                    .collect::<Vec<_>>()
            );
        }
        let agent_index = "agent_capacity_recovery_agent";
        let found = page(
            &db,
            doc! {
                "find":RECOVERY_WORK,
                "filter":{"agent_id":"agt_segment_fixture","format_version":2_i64,
                    "_id":{"$gt":format!("capacity-recovery:{:064x}", 0)}},
                "sort":{"_id":1},"projection":{"_id":1},"hint":agent_index,
                "collation":{"locale":"simple"},"limit":PAGE,"batchSize":PAGE,
                "singleBatch":true,"maxTimeMS":1000,
            },
            agent_index,
        )
        .await;
        assert_eq!(found.len(), PAGE as usize);
        assert!(found
            .windows(2)
            .all(|pair| pair[0].get_str("_id").unwrap() < pair[1].get_str("_id").unwrap()));
    }
    db.drop().await.unwrap();
}

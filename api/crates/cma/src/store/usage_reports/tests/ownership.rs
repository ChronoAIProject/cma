use super::*;
use mongodb::bson::Bson;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn live_ownership_preserves_missing_changed_and_retained_authority() {
    let store = fixture().await;
    let at = now();
    let mut original = state(1, at - 3600);
    original.owner = SubjectId("$literal-owner".into());
    seed(&store, std::slice::from_ref(&original)).await;
    for admin in [false, true] {
        let principal = scope(if admin { "operator" } else { "$literal-owner" }, admin);
        let pending = store
            .submit(
                principal.clone(),
                ReportInput::default().validate(at).unwrap(),
                at,
            )
            .await
            .unwrap();
        let lease = store
            .claim_report("report:0", "ownership-worker")
            .await
            .unwrap()
            .unwrap();
        store
            .write_report_chunk(&lease, &[fixture_row(&original)], vec![])
            .await
            .unwrap();
        store
            .finish_report(&lease, lease.report.clone(), true)
            .await
            .unwrap();
        let report = store
            .report(&principal, &pending.report_id, now())
            .await
            .unwrap();
        let id = original.sandbox_id.as_str();
        let ledger = super::super::super::sandbox_usage::STATES;
        let good = doc! {"_id":id,"owner":"$literal-owner","unrelated":"retained data"};
        for (current, allowed) in [
            (None, true),
            (Some(good.clone()), true),
            (Some(doc! {"_id":id,"owner":"transferred"}), false),
            (Some(doc! {"_id":id}), false),
            (Some(doc! {"_id":id,"owner":Bson::Null}), false),
        ] {
            store
                .collection("sandboxes")
                .delete_many(doc! {})
                .await
                .unwrap();
            if let Some(current) = current {
                store
                    .collection("sandboxes")
                    .insert_one(current)
                    .await
                    .unwrap();
            }
            let result = store.reauthorize(&report).await;
            if allowed {
                result.unwrap();
            } else {
                assert!(matches!(result, Err(MetricsError::NotFound)));
            }
        }
        store
            .collection("sandboxes")
            .delete_many(doc! {})
            .await
            .unwrap();
        for current in [
            None,
            Some(doc! {"_id":id}),
            Some(doc! {"_id":id,"owner":Bson::Null}),
            Some(doc! {"_id":id,"owner":"transferred"}),
        ] {
            store.collection(ledger).delete_many(doc! {}).await.unwrap();
            if let Some(current) = current {
                store.collection(ledger).insert_one(current).await.unwrap();
            }
            assert!(matches!(
                store.reauthorize(&report).await,
                Err(MetricsError::NotFound)
            ));
        }
        store
            .collection(ledger)
            .replace_one(doc! {"_id":id}, good)
            .upsert(true)
            .await
            .unwrap();
        store.reauthorize(&report).await.unwrap();
    }
    store.db.drop().await.unwrap();
}

fn indexed_joins(value: &Bson) -> usize {
    match value {
        Bson::Document(document) => {
            assert_ne!(document.get_str("stage").ok(), Some("COLLSCAN"));
            let own = if document.get_str("stage").ok() == Some("EQ_LOOKUP") {
                assert_eq!(document.get_str("strategy").unwrap(), "IndexedLoopJoin");
                assert_eq!(document.get_str("indexName").unwrap(), "_id_");
                1
            } else {
                0
            };
            own + document.values().map(indexed_joins).sum::<usize>()
        }
        Bson::Array(values) => values.iter().map(indexed_joins).sum(),
        _ => 0,
    }
}

fn check_winning_plans(value: &Bson) -> usize {
    match value {
        Bson::Document(document) => document
            .iter()
            .map(|(key, value)| {
                if key == "winningPlan" {
                    indexed_joins(value)
                } else {
                    check_winning_plans(value)
                }
            })
            .sum(),
        Bson::Array(values) => values.iter().map(check_winning_plans).sum(),
        _ => 0,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn live_ownership_twenty_thousand_members_keep_two_indexed_joins_and_narrow_payload() {
    let store = fixture().await;
    let report = "umr_ownership-plan";
    let count = 20_001_i64;
    let ledger = super::super::super::sandbox_usage::STATES;
    for start in (0..count).step_by(200) {
        let range = start..(start + 200).min(count);
        let rows: Vec<_> = range
            .clone()
            .map(|n| {
                doc! {
                    "_id":format!("row-{n}"),"report_id":report,
                    "sandbox_id":format!("sbx_{n:032x}"),"owner":"owner",
                    "record":{"unrelated":"x".repeat(1024)}
                }
            })
            .collect();
        store.collection(ROWS).insert_many(rows).await.unwrap();
        for collection in [ledger, "sandboxes"] {
            let documents: Vec<_> = range
                .clone()
                .map(|n| {
                    doc! {
                        "_id":format!("sbx_{n:032x}"),"owner":"owner",
                        "record":{"unrelated":"x".repeat(1024)}
                    }
                })
                .collect();
            store
                .collection(collection)
                .insert_many(documents)
                .await
                .unwrap();
        }
    }
    let pipeline = cma_am_reporting::report_membership_pipeline(report, ledger);
    let explanation = store
        .db
        .run_command(doc! {
            "explain":{"aggregate":ROWS,"pipeline":pipeline.to_vec(),"cursor":{}},
            "verbosity":"executionStats"
        })
        .await
        .unwrap();
    assert_eq!(check_winning_plans(&Bson::Document(explanation)), 2);
    assert!(store
        .collection(ROWS)
        .aggregate(pipeline.clone())
        .await
        .unwrap()
        .try_next()
        .await
        .unwrap()
        .is_none());
    // Measure before refusal filtering: an empty healthy cursor cannot prove
    // that the query discarded unrelated joined data.
    let mut joined: Vec<_> = pipeline
        .into_iter()
        .take_while(|stage| {
            !stage
                .get_document("$match")
                .is_ok_and(|query| query.contains_key("$expr"))
        })
        .collect();
    joined.push(doc! {"$group":{"_id":Bson::Null,"members":{"$sum":1},
    "bytes":{"$sum":{"$bsonSize":"$$ROOT"}}}});
    let summary = store
        .collection(ROWS)
        .aggregate(joined)
        .await
        .unwrap()
        .try_next()
        .await
        .unwrap()
        .unwrap();
    let number = |key| match summary.get(key).unwrap() {
        Bson::Int32(value) => i64::from(*value),
        Bson::Int64(value) => *value,
        _ => panic!("expected exact integer"),
    };
    assert_eq!(number("members"), count);
    let bytes = number("bytes");
    assert!(
        bytes < count * 512,
        "unrelated joined payload retained: {bytes} bytes"
    );
    store
        .collection("sandboxes")
        .update_one(
            doc! {"_id":format!("sbx_{:032x}", count - 1)},
            doc! {"$set":{"owner":"other"}},
        )
        .await
        .unwrap();
    let failures: Vec<_> = store
        .collection(ROWS)
        .aggregate(cma_am_reporting::report_membership_pipeline(report, ledger))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].get_str("_id").unwrap(),
        format!("row-{}", count - 1)
    );
    store.db.drop().await.unwrap();
}

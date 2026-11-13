use super::fixture::*;
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn full_scan_strategies_preserve_all_rows_summary_and_work_accounting() {
    let client = client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let id = AgentId("agt_scan_cooperation_fixture".into());
    let expected = entries(&id, 10_001, 256);
    let bytes = insert(&collection, &expected).await;
    let mut summaries = Vec::new();
    for (enabled, raw_batches) in [(false, false), (true, false), (true, true)] {
        let mut session = client.start_session().await.unwrap();
        let mut visited = Vec::new();
        let (result, work) = isolated_test(recorded_execution(async {
            let result = scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                10_001,
                &mut session,
                options(enabled).with_raw_batches(raw_batches),
                |entry| {
                    visited.push(entry);
                    Ok(())
                },
            )
            .await;
            assert_eq!(&snapshot(ScanCaller::Other, true)[..3], &[1, 10_001, bytes]);
            result
        }))
        .await;
        let summary = result.unwrap();
        assert_eq!(visited, expected);
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].1 > 0, enabled);
        summaries.push(summary);
    }
    assert!(summaries.iter().all(|summary| summary == &summaries[0]));
    // Physical insertion order does not override the required sequence sort.
    collection.delete_many(doc! {}).await.unwrap();
    let mut reversed = expected[..16].to_vec();
    reversed.reverse();
    insert(&collection, &reversed).await;
    for (enabled, raw_batches) in [(false, false), (true, false), (true, true)] {
        let mut session = client.start_session().await.unwrap();
        let mut sequences = Vec::new();
        scan_collection(
            ScanCaller::Other,
            &collection,
            &id,
            16,
            &mut session,
            options(enabled).with_raw_batches(raw_batches),
            |entry| {
                sequences.push(entry.seq);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(sequences, (1..=16).collect::<Vec<_>>());
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn missing_duplicate_disordered_and_corrupt_rows_keep_original_errors() {
    let client = client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let id = AgentId("agt_scan_corruption_fixture".into());
    let originals = entries(&id, 16, 32);
    for case in 0..6 {
        collection.delete_many(doc! {}).await.unwrap();
        insert(&collection, &originals).await;
        match case {
            0 => {
                collection.delete_one(doc! {"seq": 12_i64}).await.unwrap();
            }
            1 => {
                let mut duplicate = log_document(&originals[11]).unwrap();
                duplicate.insert("_id", "conflicting-duplicate-envelope");
                collection.insert_one(duplicate).await.unwrap();
            }
            2 => {
                collection
                    .update_one(doc! {"seq": 12_i64}, doc! {"$set":{"seq": 0_i64}})
                    .await
                    .unwrap();
            }
            3 => {
                collection
                    .update_one(
                        doc! {"seq": 12_i64},
                        doc! {"$set":{"record": "invalid record"}},
                    )
                    .await
                    .unwrap();
            }
            4 => {
                collection
                    .update_one(
                        doc! {"seq": 12_i64},
                        doc! {"$set":{"document_schema_version": -1_i64}},
                    )
                    .await
                    .unwrap();
            }
            5 => {
                collection
                    .update_one(doc! {"seq": 12_i64}, doc! {"$set":{"record.seq": 99_i64}})
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let mut errors = Vec::new();
        for (enabled, raw_batches) in [(false, false), (true, false), (true, true)] {
            let mut session = client.start_session().await.unwrap();
            let error = scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                16,
                &mut session,
                options(enabled).with_raw_batches(raw_batches),
                |_| Ok(()),
            )
            .await
            .unwrap_err();
            if case == 0 {
                assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::SequenceConflict)
                ));
            } else {
                assert!(matches!(
                    error,
                    TransactionLogError::Domain(SandboxStoreError::Corrupt(_))
                ));
            }
            errors.push(format!("{error:?}"));
        }
        assert!(errors.iter().all(|error| error == &errors[0]));
    }
    for (enabled, raw_batches) in [(false, false), (true, false), (true, true)] {
        let mut session = client.start_session().await.unwrap();
        let error = scan_collection(
            ScanCaller::Other,
            &collection,
            &id,
            10_129,
            &mut session,
            options(enabled).with_raw_batches(raw_batches),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            TransactionLogError::Domain(SandboxStoreError::AgentLogCapacityExceeded)
        ));
    }
    db.drop().await.unwrap();
}

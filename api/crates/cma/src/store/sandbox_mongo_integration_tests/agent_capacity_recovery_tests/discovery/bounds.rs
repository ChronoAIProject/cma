//! Profile the production consumer, including its metadata projection and cursor.
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::agent_capacity_recovery::state::RecoveryRequest;
use mongodb::bson::{Bson, Document};

fn carrier(
    serial: usize,
    phase: RecoveryPhase,
    at: &str,
) -> (Document, (String, String, RecoveryRequest)) {
    let mut stored = records::test_support::stored();
    let work = &mut stored.work;
    work.request.agent_id = AgentId(format!("consumer-bound-{serial}"));
    work.target.agent_id = work.request.agent_id.clone();
    work.target.original_open.operation_id = format!("agent:{}:open:v1", work.request.agent_id);
    work.phase = phase;
    work.ready_at = at.into();
    if phase != RecoveryPhase::Admitted {
        work.revision = 2;
        work.attempts = 1;
        if phase == RecoveryPhase::Attaching {
            work.lease = Some(AgentWorkLease {
                holder: "discovery-fixture".into(),
                fence: work.next_fence,
                expires_at: at.into(),
            });
            work.next_fence += 1;
        } else {
            work.completed_at = Some(at.into());
            stored.result.published_at = at.into();
        }
    }
    let key = work.request.key().unwrap();
    let row = doc! {"_id": &key, "format_version": 2_i64,
    "agent_id": work.request.agent_id.as_str(),
    "work": bson::serialize_to_bson(work).unwrap(),
    "result": bson::serialize_to_bson(&stored.result).unwrap()};
    (row, (at.into(), key, work.request.clone()))
}

fn number(row: &Document, field: &str) -> i64 {
    match row.get(field).unwrap() {
        Bson::Int32(value) => i64::from(*value),
        Bson::Int64(value) => *value,
        value => panic!("unexpected {field}: {value:?}"),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set with profiler access"]
async fn capacity_discovery_consumer_bounds_examined_work_across_keyset_pages() {
    let (client, name) = database("capacity_discovery_consumer").await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    crate::store::agent_log_segments::schema::migrate(&db)
        .await
        .unwrap();
    let rows = records::collection(&store);
    let profiles = db.collection::<Document>("system.profile");
    let service = RecoveryService::new(store.clone(), transactions::access(&store));
    for noise in [1, 100, 1000] {
        rows.delete_many(doc! {}).await.unwrap();
        for start in (0..noise).step_by(128) {
            let noise_rows = (start..noise.min(start + 128)).flat_map(|i| {
                [
                    carrier(i, RecoveryPhase::Completed, MINUTE).0,
                    carrier(1000 + i, RecoveryPhase::Admitted, "2026-11-11T00:02:00Z").0,
                    carrier(2000 + i, RecoveryPhase::Attaching, "2026-11-11T00:02:00Z").0,
                ]
            });
            rows.insert_many(noise_rows).await.unwrap();
        }
        let mut expectations = Vec::new();
        for (stream, phase, offset) in [
            (RecoveryStream::Admitted, RecoveryPhase::Admitted, 3000),
            (RecoveryStream::ExpiredLease, RecoveryPhase::Attaching, 4000),
        ] {
            let (documents, mut expected): (Vec<_>, Vec<_>) = (0..65)
                .map(|i| carrier(offset + i, phase, if i < 33 { NOW } else { MINUTE }))
                .unzip();
            rows.insert_many(documents).await.unwrap();
            expected.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            expectations.push((
                stream,
                expected
                    .into_iter()
                    .map(|(_, _, request)| request)
                    .collect::<Vec<_>>(),
            ));
        }
        db.run_command(doc! {"profile": 2, "slowms": 0})
            .await
            .unwrap();
        for (stream, expected) in expectations {
            let mut after = None;
            let mut observed = Vec::new();
            for count in [32, 32, 1] {
                let page = service
                    .discover(stream, after.as_ref(), MINUTE)
                    .await
                    .unwrap();
                assert_eq!(page.requests.len(), count);
                assert_eq!(page.next.is_some(), count == 32);
                observed.extend(page.requests);
                after = page.next;
            }
            assert_eq!(observed, expected);
        }
        db.run_command(doc! {"profile": 0}).await.unwrap();
        let operations: Vec<Document> = profiles.find(doc! {
            "ns": format!("{name}.agent_capacity_recovery_work"),
            "$or": [{"command.find": {"$exists": true}}, {"command.getMore": {"$exists": true}}],
        }).sort(doc! {"ts": 1}).await.unwrap().try_collect().await.unwrap();
        // Require actual-consumer samples, including all continuation pages.
        assert_eq!(operations.len(), 6, "noise={noise}: {operations:?}");
        let mut counts: Vec<_> = operations
            .iter()
            .map(|operation| number(operation, "nreturned"))
            .collect();
        counts.sort_unstable();
        assert_eq!(counts, [1, 1, 32, 32, 32, 32]);
        for operation in &operations {
            let count = number(operation, "nreturned");
            let command = operation.get_document("command").unwrap();
            assert_eq!(
                command.get_str("find").unwrap(),
                "agent_capacity_recovery_work"
            );
            assert_eq!(number(command, "limit"), 32);
            assert_eq!(number(command, "batchSize"), 32);
            assert_eq!(number(command, "maxTimeMS"), 1000);
            assert!(matches!(
                command.get_str("hint").unwrap(),
                "agent_capacity_recovery_due" | "agent_capacity_recovery_expired"
            ));
            assert!(
                number(operation, "docsExamined") <= count,
                "noise={noise}: {operation:?}"
            );
            assert!(
                number(operation, "keysExamined") <= 2 * (count + 1),
                "noise={noise}: {operation:?}"
            );
            assert!(number(operation, "responseLength") <= 128 * 1024);
            assert!(!operation
                .get_str("planSummary")
                .unwrap()
                .contains("COLLSCAN"));
            assert_ne!(operation.get_bool("hasSortStage").ok(), Some(true));
            eprintln!(
                "capacity discovery noise={noise} returned={count} keys={} docs={} bytes={}",
                number(operation, "keysExamined"),
                number(operation, "docsExamined"),
                number(operation, "responseLength")
            );
        }
        profiles.drop().await.unwrap();
    }
    db.drop().await.unwrap();
}

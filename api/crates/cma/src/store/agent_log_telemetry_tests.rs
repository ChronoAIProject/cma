use super::*;

fn values(totals: &Totals) -> [u64; 3] {
    [&totals.scans, &totals.documents, &totals.bytes].map(|v| v.load(Ordering::Relaxed))
}

#[test]
fn completed_empty_and_nonempty_scans_have_exact_raw_lengths() {
    let totals = Arc::new([Totals::default(), Totals::default()]);
    let raw = mongodb::bson::rawdoc! { "value": "test payload" };
    let length = raw.as_bytes().len();
    {
        let mut scan = Scan::with_totals(totals.clone());
        scan.document(length);
        scan.document(length);
        scan.complete();
    }
    {
        let mut scan = Scan::with_totals(totals.clone());
        scan.complete();
    }
    assert_eq!(values(&totals[0]), [0; 3]);
    assert_eq!(values(&totals[1]), [2, 2, (2 * length) as u64]);
}

#[tokio::test]
async fn cancelled_scans_retain_partial_work_and_never_report_success() {
    let totals = Arc::new([Totals::default(), Totals::default()]);
    let work = async {
        let mut scan = Scan::with_totals(totals.clone());
        scan.document(123);
        std::future::pending::<()>().await;
        scan.complete();
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), work)
            .await
            .is_err()
    );
    assert_eq!(values(&totals[0]), [1, 1, 123]);
    assert_eq!(values(&totals[1]), [0; 3]);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn real_complete_scan_counts_success_corruption_and_failed_reads() {
    use super::super::{agent_log::log_document, agent_log_integrity::scan_collection};
    use layer2_core::{
        ids::AgentId,
        sandbox::{AgentLogEntry, AgentLogKind},
    };
    use mongodb::{
        bson::{doc, Document},
        Client,
    };
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!("cma_scan_{}", uuid::Uuid::new_v4().simple()));
    let collection = database.collection::<Document>("agent_log");
    let id = AgentId("agt_scan_fixture".into());
    let document = log_document(&AgentLogEntry {
        agent_id: id.clone(),
        seq: 1,
        appended_at: "2026-11-08T00:00:00Z".into(),
        kind: AgentLogKind::Deleted,
    })
    .unwrap();
    // log_document supplies the authoritative _id; Mongo adds no bytes.
    let bytes = document.to_vec().unwrap().len() as u64;
    collection.insert_one(document).await.unwrap();
    let totals = Arc::new([Totals::default(), Totals::default()]);
    TEST_TOTALS
        .scope(totals.clone(), async {
            let mut session = client.start_session().await.unwrap();
            assert!(
                scan_collection(&collection, &id, 1, &mut session, false, |_| Ok(()))
                    .await
                    .is_ok()
            );
            assert_eq!(values(&totals[1]), [1, 1, bytes]);
            // A missing tail retains the bytes read, but does not count a completed validation.
            assert!(
                scan_collection(&collection, &id, 2, &mut session, false, |_| Ok(()))
                    .await
                    .is_err()
            );
            assert_eq!(values(&totals[0]), [1, 1, bytes]);
            collection
                .update_one(doc! {}, doc! {"$set": {"seq": 7_i64}})
                .await
                .unwrap();
            assert!(
                scan_collection(&collection, &id, 1, &mut session, false, |_| Ok(()))
                    .await
                    .is_err()
            );
            assert_eq!(values(&totals[0]), [2, 2, 2 * bytes]);
            let invalid = database.collection::<Document>("invalid\0collection");
            assert!(
                scan_collection(&invalid, &id, 1, &mut session, false, |_| Ok(()))
                    .await
                    .is_err()
            );
            assert_eq!(values(&totals[0]), [3, 2, 2 * bytes]);
        })
        .await;
    database.drop().await.unwrap();
}

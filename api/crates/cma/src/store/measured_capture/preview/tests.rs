use super::*;
use crate::store::measured_capture::{tests::observation_at, Capture, Client, COLLECTION};
use mongodb::options::ClientOptions;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn measured_preview_preserves_disputed_positions_and_refuses_incomplete_scans() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let reader = Client::with_uri_str(&uri).await.unwrap();
    let writer = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_preview_{}", uuid::Uuid::new_v4().simple());
    let database = reader.database(&name);
    let preview = MeasuredCapture::new(&database);
    let recorder = MeasuredCapture::new(&writer.database(&name));
    let window = Range { from: 20, to: 40 };
    let first = observation_at("owner-a", 1, 21_000, 10.0);
    let middle = observation_at("owner-a", 2, 30_000, 20.0);
    let last = observation_at("owner-a", 1, 39_000, 40.0);
    let anchor = first.physical_sample_id();
    for row in [&last, &first, &middle] {
        assert_eq!(recorder.capture(row).await.unwrap(), Capture::Inserted);
    }
    let before = preview.preview_bounded(anchor, window, 3).await.unwrap();
    let intervals = before["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 2);
    assert_eq!(intervals[0]["quantity"]["byte_milliseconds"], "90000");
    assert_eq!(intervals[1]["quantity"]["byte_milliseconds"], "180000");
    assert_eq!(
        intervals[0]["evidence_sha256"],
        json!([
            first.evidence_sha256().unwrap(),
            middle.evidence_sha256().unwrap()
        ])
    );
    assert!(before["snapshot"]["seconds"].as_u64().unwrap() > 0);
    assert!(matches!(
        preview.preview_bounded(anchor, window, 2).await,
        Err(CaptureError::Unavailable)
    ));

    let rival = observation_at("owner-b", 3, 30_000, 20.0);
    assert_eq!(recorder.capture(&rival).await.unwrap(), Capture::Disputed);
    let disputed = preview.preview_bounded(anchor, window, 4).await.unwrap();
    let intervals = disputed["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 2);
    assert!(intervals
        .iter()
        .all(|pair| pair["reason"] == "disputed" && pair.get("quantity").is_none()));
    assert_eq!(intervals[0]["to_ms"], intervals[1]["from_ms"]);
    assert_eq!(
        intervals[0]["evidence_sha256"][1],
        intervals[1]["evidence_sha256"][0]
    );

    database
        .collection::<Document>(COLLECTION)
        .insert_one(doc! {"_id": "unknown-custody-envelope", "payload": "invalid"})
        .await
        .unwrap();
    assert!(matches!(
        preview.preview(anchor, window).await,
        Err(CaptureError::Corrupt)
    ));
    database.drop().await.unwrap();
}

// The two cursor batches must use the same snapshot even while a competing
// observation is committed by another client.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn measured_preview_keeps_one_snapshot_across_concurrent_dispute_and_get_more() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let name = format!("measured_snapshot_{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    let writer = MeasuredCapture::new(&admin.database(&name));
    let reader = MeasuredCapture::new(&client.database(&name));
    let first = observation_at("owner-a", 1, 21_000, 10.0);
    let middle = observation_at("owner-a", 1, 30_000, 20.0);
    let last = observation_at("owner-a", 1, 39_000, 40.0);
    let anchor = first.physical_sample_id().to_owned();
    let mut samples = vec![first, middle, last];
    samples.extend((20_000..20_064).map(|timestamp| observation_at("owner-a", 1, timestamp, 7.0)));
    let mut existing_ids: Vec<_> = samples
        .iter()
        .map(|sample| sample.physical_sample_id())
        .collect();
    existing_ids.sort_unstable();
    let unread = (30_001..39_000)
        .map(|timestamp| observation_at("owner-a", 1, timestamp, 30.0))
        .find(|sample| sample.physical_sample_id() > existing_ids[63])
        .expect("fixture has a sample that sorts in the unread cursor range");
    let rows = samples.iter().map(|sample| {
        let physical = sample.physical_sample_id();
        crate::store::measured_capture::encode(
            &crate::store::measured_capture::sample_id(physical),
            physical,
            "sample",
            &sample.evidence_sha256().unwrap(),
            &serde_json::to_string(sample).unwrap(),
        )
    });
    writer.rows.insert_many(rows).await.unwrap();

    let admin_db = admin.database("admin");
    let configured = admin_db.run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 1},
        "data": {"failCommands": ["getMore"], "appName": &name, "blockConnection": true, "blockTimeMS": 10_000},
    }).await.unwrap();
    let entered = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap()
        + 1;
    let window = Range { from: 21, to: 40 };
    let in_progress = tokio::spawn({
        let anchor = anchor.clone();
        let reader = reader.clone();
        async move { reader.preview(&anchor, window).await }
    });
    let barrier = admin_db
        .run_command(doc! {
            "waitForFailPoint": "failCommand", "timesEntered": entered, "maxTimeMS": 8_000,
        })
        .await;
    let disputed = writer
        .capture(&observation_at("owner-b", 1, 30_000, 20.0))
        .await;
    let added = writer.capture(&unread).await;
    // MongoDB waits for the blocked getMore when disabling the failpoint.
    let snapshot_pending = !in_progress.is_finished();
    admin_db
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    barrier.unwrap();
    assert_eq!(disputed.unwrap(), Capture::Disputed);
    assert_eq!(added.unwrap(), Capture::Inserted);
    assert!(snapshot_pending);
    let before = in_progress.await.unwrap().unwrap();
    let intervals = before["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 2);
    assert!(intervals.iter().all(|pair| pair["status"] == "measured"));
    let after = reader.preview(&anchor, window).await.unwrap();
    let intervals = after["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 3);
    assert_eq!(intervals[0]["reason"], "disputed");
    assert_eq!(intervals[1]["reason"], "disputed");
    assert_eq!(intervals[2]["status"], "measured");
    assert_eq!(
        intervals[2]["evidence_sha256"][0],
        unread.evidence_sha256().unwrap()
    );
    admin.database(&name).drop().await.unwrap();
}

use super::*;
use layer2_core::{SandboxId, SubjectId};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client, IndexModel,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable MongoDB"]
async fn dense_allocation_bucket_page_bounds_native_cursors_and_preserves_edges() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable MongoDB required");
    let finds = Arc::new(Mutex::new(Vec::new()));
    let recorded = finds.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.command.get_str("find").ok() == Some(COLLECTION) {
            let ids = event
                .command
                .get_document("filter")
                .unwrap()
                .get_document("sandbox_id")
                .unwrap()
                .get_array("$in")
                .unwrap();
            recorded.lock().unwrap().push(ids.len());
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!(
        "allocation_cursor_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let collection = db.collection::<Document>(COLLECTION);
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"sandbox_id":1, "day":1})
                .build(),
        )
        .await
        .unwrap();

    let day = 100 * DAY;
    let end = day + 30 * DAY;
    let mut states: Vec<_> = (0..80)
        .map(|id| SandboxUsage {
            sandbox_id: SandboxId(format!("sbx_{id:032x}")),
            owner: SubjectId("owner-fixture".into()),
            name: "Synthetic".into(),
            status: "active".into(),
            allocation: Allocation {
                running: true,
                cpu_millis: 1000,
                memory_mib: 1024,
                disk_gib: 16,
            },
            created_at: day,
            tracking_started_at: day,
            last_observed_at: day,
            source_seq: 1,
            source_revision: 1,
            revision: 1,
            has_started: true,
            complete_lifecycle: true,
            totals: Default::default(),
        })
        .collect();
    for state in &states {
        let docs = (day..end)
            .step_by(DAY as usize)
            .map(|from| {
                let to = from + DAY;
                build_covered(state, &[], Range { from, to }, to)
                    .unwrap()
                    .document()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        collection.insert_many(docs).await.unwrap();
    }
    // Multiple inputs for one Sandbox still produce a single set of buckets.
    states.push(states[0].clone());
    let range = Range {
        from: day + HOUR,
        to: end - HOUR,
    };
    let mut session = client.start_session().await.unwrap();
    let page = read_bucket_page(
        collection,
        &mut session,
        &states,
        range,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(*finds.lock().unwrap(), [32, 32, 16, 32, 32, 16]);
    assert_eq!(page.len(), 80 * 30);
    for state in states.iter().take(80) {
        for date in (day..end).step_by(DAY as usize) {
            let bucket = &page[&(state.sandbox_id.to_string(), date)];
            let range = Range {
                from: date,
                to: date + DAY,
            };
            assert_eq!(
                bucket.total().projection.period,
                allocation::project(state, &[], range).period
            );
            assert_eq!(bucket.has_boundary(), date == day || date == end - DAY);
        }
    }
    db.drop().await.unwrap();
}

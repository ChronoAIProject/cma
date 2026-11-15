use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_summary_reads_are_bounded_projected_and_never_count_or_write() {
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let seen = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            seen.lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_activity_commands_{}", uuid::Uuid::new_v4().simple());
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    store.append(&row(1)).await.unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(
        store
            .list_summaries(&ActivityQuery {
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let observed = commands.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "{observed:?}");
    assert_eq!(observed[0].0, "find");
    let query = &observed[0].1;
    assert_eq!(query.get_i64("limit").unwrap(), 2);
    // The driver requests one extra batch slot to avoid an empty getMore at the limit.
    assert_eq!(query.get_i32("batchSize").unwrap(), 3);
    assert_eq!(query.get_i32("maxTimeMS").unwrap(), 5000);
    assert!(!query.contains_key("skip") && !query.contains_key("hint"));
    let projection = query.get_document("projection").unwrap();
    assert_eq!(projection.len(), 9);
    assert!(!projection.contains_key("record.detail") && !projection.contains_key("record"));
    assert!(
        projection.contains_key("_id")
            && projection.contains_key("recorded_at")
            && projection.contains_key("document_schema_version")
    );
    commands.lock().unwrap().clear();
    assert!(store
        .list_summaries(&ActivityQuery {
            cursor: Some("invalid".into()),
            ..Default::default()
        })
        .await
        .is_err());
    assert!(commands.lock().unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

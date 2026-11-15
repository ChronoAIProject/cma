use super::*;
use layer2_core::{ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityStore};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn activity_batch_is_majority_unordered_idempotent_and_cursor_readable() {
    let (admin, name) = database().await;
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "insert" {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86_400))
        .await
        .unwrap();
    let records: Vec<_> = (0..5)
        .map(|_| {
            let mut record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded);
            record.at = "2001-01-01T00:00:00Z".into();
            record
        })
        .collect();
    store.append_many(&[]).await.unwrap();
    assert!(commands.lock().unwrap().is_empty());
    store.append_many(&records[..3]).await.unwrap();
    // A lost acknowledgement retries existing IDs while still inserting new IDs.
    store.append_many(&records).await.unwrap();
    store.append_many(&records).await.unwrap();
    let documents = admin
        .database(&name)
        .collection::<Document>(ACTIVITY)
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(documents.len(), records.len());
    for document in &documents {
        assert!(
            document
                .get_datetime("recorded_at")
                .unwrap()
                .timestamp_millis()
                > 1_000_000_000_000
        );
    }
    let mut query = ActivityQuery {
        limit: 2,
        ..ActivityQuery::default()
    };
    let mut read = Vec::new();
    loop {
        let page = store.list(&query).await.unwrap();
        read.extend(page.items);
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    read.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    let mut expected = records.clone();
    expected.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    assert_eq!(read, expected);
    {
        let commands = commands.lock().unwrap();
        assert_eq!(commands.len(), 3);
        for command in commands.iter() {
            assert!(!command.get_bool("ordered").unwrap());
            assert_eq!(
                command
                    .get_document("writeConcern")
                    .unwrap()
                    .get_str("w")
                    .unwrap(),
                "majority"
            );
        }
    }
    // Non-duplicate failures must not be mistaken for a successful retry.
    admin
        .database(&name)
        .run_command(doc! {
            "collMod": ACTIVITY, "validator": { "record.action": "create" },
            "validationLevel": "strict", "validationAction": "error"
        })
        .await
        .unwrap();
    let mut mixed = records;
    mixed.push(ActivityRecord::new(
        ActivityAction::Read,
        ActivityOutcome::Succeeded,
    ));
    assert!(store.append_many(&mixed).await.is_err());
    admin.database(&name).drop().await.unwrap();
}

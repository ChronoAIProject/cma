use super::*;
use crate::store::mongo_read_telemetry::{isolated_test, snapshot};
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn fixed_sealed_page_has_fixed_source_bytes_and_bounded_queries_as_history_grows() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command_name);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let mut baseline = None;
    for closed in [1, 10, 100, 1000] {
        let name = format!("cma_segments_bounds_{}", uuid::Uuid::new_v4().simple());
        schema::migrate(&client.database(&name)).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = segmented(&store, closed).await;
        commands.lock().unwrap().clear();
        let consumed = isolated_test(async {
            let page = store.agent_log_range(&agent.id, 0, 2, 2).await.unwrap();
            assert_eq!(page, entries[..2]);
            snapshot("agent_log")
        })
        .await;
        let queries = commands.lock().unwrap().clone();
        assert!(
            queries.iter().all(|n| n == "find" || n == "getMore"),
            "{queries:?}"
        );
        assert!(queries.len() <= 10, "{closed}: {queries:?}");
        assert_eq!(&consumed[..3], &[1, 0, 2]);
        if let Some(previous) = baseline {
            assert_eq!(consumed, previous);
        } else {
            baseline = Some(consumed);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn failed_leaf_validation_accounts_for_consumed_source_without_a_lifetime_scan() {
    let (client, name) = database("read_failure").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":2_i64},
            doc! {"$set":{"record.appended_at":"2026-11-02T00:00:00Z"}},
        )
        .await
        .unwrap();
    isolated_test(async {
        assert!(store.agent_log_range(&agent.id, 0, 2, 2).await.is_err());
        let consumed = snapshot("agent_log");
        assert_eq!(&consumed[..3], &[1, 1, 2]);
        assert!(consumed[3] > 0);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn global_history_exceeds_physical_segment_budget_without_a_lifetime_vector() {
    let (client, name) = database("lifetime").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented_with_width(&store, 2, 10_000, 2).await;
    assert!(agent.conversation.last_seq > MAX_ENTRIES as u64);
    assert_eq!(
        store
            .agent_log_range(&agent.id, 9_998, 10_003, 5)
            .await
            .unwrap(),
        entries[9_998..10_003],
    );
    let mut seen = Vec::new();
    crate::store::agent_log_store::visit_page(
        &store,
        &agent.id,
        19_999,
        20_002,
        3,
        true,
        &mut |entry| {
            seen.push(entry);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(seen, entries[19_998..]);
    assert!(matches!(
        store
            .agent_log_prefix(&agent.id, agent.conversation.last_seq)
            .await,
        Err(layer2_core::sandbox::store::SandboxStoreError::AgentLogCapacityExceeded),
    ));
    client.database(&name).drop().await.unwrap();
}

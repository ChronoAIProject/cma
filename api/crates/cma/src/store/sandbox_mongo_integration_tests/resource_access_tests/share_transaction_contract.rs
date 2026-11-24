use super::*;
use crate::store::agent_log_segments as segments;
use futures_util::TryStreamExt;
use mongodb::{
    bson::RawDocumentBuf,
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

async fn rows(client: &Client, name: &str, collections: &[&str]) -> Vec<Vec<RawDocumentBuf>> {
    let database = client.database(name);
    let mut snapshot = Vec::new();
    for collection in collections {
        snapshot.push(
            database
                .collection::<RawDocumentBuf>(collection)
                .find(doc! {})
                .sort(doc! {"_id": 1_i32})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    snapshot
}

const HISTORY: &[&str] = &[
    "agent_log",
    segments::HEADS,
    segments::SEGMENTS,
    segments::PROJECTIONS,
    segments::EVIDENCE,
];
const CUSTODY: &[&str] = &[
    "sandboxes",
    "workspaces",
    "worktrees",
    "agents",
    "resource_policies",
    "resource_shares",
    "resource_share_commands",
];

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn share_transaction_contract_accepts_valid_segmented_agent_without_rewriting_history() {
    let (client, name) = database("share_segmented_fence").await;
    let (store, target) = fixture(&client, &name).await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let agent = before.lineage.agent.clone().unwrap();
    let logs = store.agent_log(&agent.id, 0, 20).await.unwrap();
    segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    store
        .agents
        .delete_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap();
    segments::mongo_tests::fixtures::install(&store, agent.clone(), logs, 0, 2).await;
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        before
    );
    let history = rows(&client, &name, HISTORY).await;
    assert_eq!(history[0].len(), 2);
    assert_eq!(history[1].len(), 1);
    let request = mutation(&before, "segment-receipt-fixture", ShareMode::View);
    let outcome = store.mutate_share(request.clone()).await;
    let replay = if outcome.is_ok() {
        Some(store.mutate_share(request).await)
    } else {
        None
    };
    let saved_history = rows(&client, &name, HISTORY).await;
    let agent = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let receipts = store
        .resource_share_commands
        .count_documents(doc! {})
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
    let receipt = outcome.expect("valid segmented Agent should fence in the share transaction");
    assert_eq!(replay, Some(Ok(receipt)));
    assert_eq!(saved_history, history);
    assert_eq!(agent.get_i64("resource_share_guard").unwrap(), 1);
    assert_eq!(receipts, 1);
}

async fn overloaded_abort(uncertain: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let observer = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("share_retry_{}", uuid::Uuid::new_v4().simple());
    let (seed, target) = fixture(&observer, &name).await;
    let before = seed.resource_access_snapshot(&target).await.unwrap();
    let app = format!("share-fault-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let starts = Arc::new(AtomicUsize::new(0));
    let captured = starts.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_bool("startTransaction").ok() == Some(true) {
                captured.fetch_add(1, Ordering::Relaxed);
            }
        }
    }));
    let worker = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&worker, &name).await.unwrap();
    let before_rows = rows(&observer, &name, CUSTODY).await;
    let mut labels = vec!["TransientTransactionError"];
    if uncertain {
        labels.push("UnknownTransactionCommitResult");
    }
    observer
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand",
            "mode": {"times": 1_i32}, "data": {"appName": app,
                "failCommands": ["insert"], "errorCode": 11_000_i32, "errorLabels": labels},
        })
        .await
        .unwrap();
    let request = mutation(&before, "share-fault-receipt", ShareMode::View);
    let result = store.mutate_share(request.clone()).await;
    observer
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let observed_starts = starts.load(Ordering::Relaxed);
    let retained_rows = rows(&observer, &name, CUSTODY).await;
    let stored = seed
        .share_mutation_receipt(&request.operation_id())
        .await
        .unwrap();
    observer.database(&name).drop().await.unwrap();
    if uncertain {
        assert_eq!(result, Err(ResourceAccessStoreError::Unavailable));
        assert_eq!(observed_starts, 1);
        assert!(stored.is_none());
        assert_eq!(retained_rows, before_rows);
    } else {
        assert_eq!(observed_starts, 2);
        assert_eq!(stored, Some(result.unwrap()));
        assert_eq!(retained_rows.last().unwrap().len(), 1);
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn share_transaction_contract_does_not_repeat_uncertain_body() {
    overloaded_abort(true).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn share_transaction_contract_retries_a_confirmed_abort_once() {
    overloaded_abort(false).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn share_transaction_contract_wrong_owner_never_changes_custody() {
    let (client, name) = database("share_owner_fence").await;
    let (store, target) = fixture(&client, &name).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let before = rows(&client, &name, CUSTODY).await;
    let mut request = mutation(&snapshot, "wrong-owner-fixture", ShareMode::View);
    let foreign_owner = SubjectId::new("foreign-owner");
    request.owner = ResourceOwner::Person {
        subject: foreign_owner.clone(),
    };
    request.actor = ResourceActor::Human {
        subject: foreign_owner,
    };
    let result = store.mutate_share(request).await;
    let after = rows(&client, &name, CUSTODY).await;
    client.database(&name).drop().await.unwrap();
    assert_eq!(result, Err(ResourceAccessStoreError::NotFound));
    assert_eq!(after, before);
}

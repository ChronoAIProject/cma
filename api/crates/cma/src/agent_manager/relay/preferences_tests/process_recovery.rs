use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use futures_util::{FutureExt, TryStreamExt};
use layer2_core::agent_ownership::{OwnershipStore, DEFAULT_LEASE_MS};
use mongodb::bson::{doc, Document};
use std::{panic::AssertUnwindSafe, time::Duration};

#[path = "process_recovery/child.rs"]
mod child;
use child::ReplicaProcess;

const FIRST_TITLE: &str = "Title acknowledged before process loss";
const NEXT_TITLE: &str = "Title acknowledged by the successor";
const FIRST_REQUEST: &str = "00000000-0000-4000-8000-000000000101";
const NEXT_REQUEST: &str = "00000000-0000-4000-8000-000000000102";
const STALE_REQUEST: &str = "00000000-0000-4000-8000-000000000103";

#[tokio::test]
#[ignore = "requires a disposable loopback Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn rename_replays_after_owner_process_loss() {
    child::require_local_mongo().await;
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let database = mongo.database(&name);
    let mut replicas = Vec::with_capacity(2);
    let primary = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(60), async {
        seed(&durable).await;
        let ownership = MongoAgentOwnership::open(&mongo, &name).await.unwrap();
        for _ in 0..2 {
            // Retain even a partially started child for explicit teardown.
            replicas.push(ReplicaProcess::spawn(&name));
            replicas.last_mut().unwrap().read_ready().await;
        }
        verify_recovery(&database, &ownership, &mut replicas).await;
    }))
    .catch_unwind()
    .await;
    let mut children_stopped = true;
    for replica in &mut replicas {
        children_stopped &= replica.terminate().await.is_ok();
    }
    let database_removed = children_stopped
        && tokio::time::timeout(Duration::from_secs(5), database.drop())
            .await
            .is_ok_and(|result| result.is_ok());
    if !children_stopped || !database_removed {
        eprintln!(
            "preferences process fixture cleanup children_stopped={children_stopped} database_removed={database_removed}"
        );
    }
    match primary {
        Ok(Ok(())) => assert!(
            children_stopped && database_removed,
            "fixture cleanup failed"
        ),
        Ok(Err(_)) => panic!("preferences process fixture exceeded its runtime bound"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

async fn verify_recovery(
    database: &mongodb::Database,
    ownership: &MongoAgentOwnership,
    replicas: &mut [ReplicaProcess],
) {
    assert_ne!(replicas[0].pid(), replicas[1].pid());
    assert_ne!(replicas[0].address(), replicas[1].address());
    let agent_id = AgentId(AGENT.into());
    let membership = ownership.membership().await.unwrap();
    assert_eq!(membership.members.len(), 2);
    let preferred = candidates(&agent_id, &membership, None).unwrap();
    let owner = replicas
        .iter()
        .position(|replica| replica.instance() == preferred[0].instance_id)
        .unwrap();
    let peer = 1 - owner;
    let original = replicas[owner].acquire().await.unwrap();
    assert_eq!(original.instance_id, replicas[owner].instance());
    assert_eq!(
        ownership.owner(&agent_id).await.unwrap().unwrap().token,
        original
    );
    assert!(replicas[peer].acquire().await.is_none());

    let first = rename(
        &replicas[peer],
        FIRST_REQUEST,
        "process-first",
        FIRST_TITLE,
        0,
    )
    .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first.1["title"], FIRST_TITLE);
    assert_eq!(first.1["revision"], 1);
    assert!(replicas[peer].take_scopes().await.is_empty());
    assert_eq!(
        replicas[owner].take_scopes().await,
        vec![Some(original.clone())]
    );
    let original_events = title_events(database).await;
    assert_eq!(original_events.len(), 1);
    assert_eq!(original_events[0].1, FIRST_TITLE);

    let status = replicas[owner].terminate().await.unwrap();
    assert!(!status.success(), "the holder must exit abruptly");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    let successor = tokio::time::timeout(Duration::from_millis(DEFAULT_LEASE_MS), async {
        loop {
            if let Some(authority) = replicas[peer].acquire().await {
                break authority;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("surviving process must acquire within one production lease");
    assert_eq!(successor.instance_id, replicas[peer].instance());
    assert!(successor.epoch > original.epoch);
    assert_eq!(
        ownership.owner(&agent_id).await.unwrap().unwrap().token,
        successor
    );

    let read = client()
        .get(preferences_url(&replicas[peer]))
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.json::<Value>().await.unwrap(), first.1);
    assert_eq!(
        rename(
            &replicas[peer],
            FIRST_REQUEST,
            "process-first",
            FIRST_TITLE,
            0
        )
        .await,
        first
    );
    assert_eq!(title_events(database).await, original_events);
    let next = rename(&replicas[peer], NEXT_REQUEST, "process-next", NEXT_TITLE, 1).await;
    assert_eq!(next.0, StatusCode::OK);
    assert_eq!(next.1["title"], NEXT_TITLE);
    assert_eq!(next.1["revision"], 2);
    let stale = rename(
        &replicas[peer],
        STALE_REQUEST,
        "process-stale",
        "Stale title",
        0,
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    assert_eq!(stale.1["settings_update_reason"], "stale_revision");
    assert_eq!(replicas[peer].take_scopes().await, vec![Some(successor); 3]);
    let events = title_events(database).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], original_events[0]);
    assert!(events[1].0 > events[0].0);
    assert_eq!(events[1].1, NEXT_TITLE);
}

async fn rename(
    replica: &ReplicaProcess,
    request_id: &str,
    key: &str,
    title: &str,
    expected_revision: u64,
) -> (StatusCode, Value) {
    let response = client()
        .patch(preferences_url(replica))
        .header("x-request-id", request_id)
        .header("idempotency-key", key)
        .header("prefer", "cma-settings-reasons")
        .json(&json!({ "title": title, "expected_revision": expected_revision }))
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}

fn preferences_url(replica: &ReplicaProcess) -> String {
    format!(
        "http://{}/api/v1/agents/{AGENT}/preferences",
        replica.address()
    )
}

async fn title_events(database: &mongodb::Database) -> Vec<(i64, String)> {
    let rows: Vec<Document> = database
        .collection("agent_log")
        .find(doc! { "agent_id": AGENT, "record.kind.type": "title_set" })
        .sort(doc! { "seq": 1 })
        .limit(3)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    rows.into_iter()
        .map(|row| {
            let title = row
                .get_document("record")
                .unwrap()
                .get_document("kind")
                .unwrap()
                .get_str("title")
                .unwrap();
            (row.get_i64("seq").unwrap(), title.to_owned())
        })
        .collect()
}

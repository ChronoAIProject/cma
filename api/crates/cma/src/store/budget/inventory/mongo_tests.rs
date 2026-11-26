use super::*;
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::{ClientOptions, Credential},
};
use std::sync::{Arc, Mutex};
use tests::{agent, document, saved};

#[path = "mongo_fixture.rs"]
mod fixture;
use fixture::Fixture;

#[path = "mongo_admission.rs"]
mod admission;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn empty_inventory_is_scoped_and_missing_collections_are_not_initialized() {
    let fixture = Fixture::new().await;
    let report = fixture.observe(Limits::default(), &()).await.unwrap();
    assert_eq!(report.selected, 0);
    let body: Value = serde_json::from_slice(&report.bytes).unwrap();
    assert_eq!(body["financial_reconciliation"], "not_performed");
    assert!(body["excluded"].as_array().unwrap().len() >= 5);
    assert!(
        observe(&fixture.admin, &fixture.database, Limits::default(), &())
            .await
            .is_err()
    );
    fixture.budget().drop().await.unwrap();
    assert!(matches!(
        fixture.observe(Limits::default(), &()).await,
        Err("turn_inventory_existing_collections_required")
    ));
    assert!(!fixture
        .database
        .list_collection_names()
        .await
        .unwrap()
        .contains(&super::super::COLLECTION.to_owned()));
    fixture.assert_reads();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn inventory_retains_unknown_obligations_and_excludes_transfer_copies() {
    let fixture = Fixture::new().await;
    let mut first = agent("a");
    first.conversation.active_turn = None;
    first.status = layer2_core::sandbox::AgentStatus::Ready;
    let mut retained = saved(&first);
    retained.reservation.reserved_units = (1_u64 << 53) + 1;
    fixture.insert(&first, &retained).await;
    let second = agent("b");
    let mut settled = saved(&second);
    settled.reservation.settled = true;
    fixture.insert(&second, &settled).await;
    let mut copy = document(&settled);
    copy.insert("_id", "command:retained-copy");
    copy.get_document_mut("record")
        .unwrap()
        .get_document_mut("reservation")
        .unwrap()
        .insert("settled", false);
    fixture.budget().insert_one(copy).await.unwrap();
    fixture
        .budget()
        .insert_one(doc! {"_id": "request:excluded", "unrecognized": true})
        .await
        .unwrap();
    let report = fixture.observe(Limits::default(), &()).await.unwrap();
    let body: Value = serde_json::from_slice(&report.bytes).unwrap();
    assert_eq!(body["selected"], 2);
    assert_eq!(body["settled"], 1);
    assert_eq!(body["unresolved"].as_array().unwrap().len(), 1);
    assert_eq!(body["unresolved"][0]["linkage"], "no_active_turn");
    assert_eq!(
        body["unresolved"][0]["saved"]["reservation"]["reserved_units"],
        "9007199254740993"
    );
    assert_eq!(body["recovery_authorized"], false);
    assert_eq!(body["eof"], true);
    assert_eq!(fixture.budget().count_documents(doc! {}).await.unwrap(), 4);
    assert_eq!(
        fixture
            .budget()
            .find_one(doc! {"_id": "turn:a:2"})
            .await
            .unwrap()
            .unwrap(),
        document(&retained)
    );
    fixture.assert_reads();
    fixture.cleanup().await;
}

struct Mutate {
    database: Database,
    replace_collection: bool,
}
#[async_trait::async_trait]
impl PageHook for Mutate {
    async fn after_page(&self, page: usize) -> Result<()> {
        if page != 1 {
            return Ok(());
        }
        if self.replace_collection {
            self.database
                .collection::<Document>("agents")
                .drop()
                .await
                .unwrap();
            self.database.create_collection("agents").await.unwrap();
        } else {
            self.database
                .collection::<Document>(super::super::COLLECTION)
                .update_many(doc! {}, doc! {"$set": {"record.reservation.settled": true}})
                .await
                .unwrap();
            self.database
                .collection::<Document>("agents")
                .delete_many(doc! {})
                .await
                .unwrap();
            self.database
                .collection(super::super::COLLECTION)
                .insert_one(document(&saved(&agent("later"))))
                .await
                .unwrap();
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn inventory_pages_and_agent_lookups_share_one_snapshot_cut() {
    let fixture = Fixture::new().await;
    for id in ["a", "b"] {
        let agent = agent(id);
        fixture.insert(&agent, &saved(&agent)).await;
    }
    let report = fixture
        .observe(
            Limits {
                page_size: 1,
                ..Limits::default()
            },
            &Mutate {
                database: fixture.database.clone(),
                replace_collection: false,
            },
        )
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&report.bytes).unwrap();
    assert_eq!(body["selected"], 2);
    assert_eq!(body["settled"], 0);
    assert_eq!(body["linkages"]["matching_active_turn"], 2);
    assert!(body["snapshot"]["time"].as_u64().unwrap() > 0);
    assert_eq!(fixture.budget().count_documents(doc! {}).await.unwrap(), 3);
    fixture.assert_reads();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn inventory_refuses_collection_replacement_and_incomplete_bounds() {
    let fixture = Fixture::new().await;
    let agent = agent("a");
    fixture.insert(&agent, &saved(&agent)).await;
    for limits in [
        Limits {
            rows: 0,
            ..Limits::default()
        },
        Limits {
            page_size: 1,
            pages: 1,
            ..Limits::default()
        },
        Limits {
            source_bytes: 1,
            ..Limits::default()
        },
        Limits {
            output_bytes: 1,
            ..Limits::default()
        },
    ] {
        assert!(matches!(fixture.observe(limits, &()).await, Err(LIMIT)));
    }
    assert!(matches!(
        fixture
            .observe(
                Limits {
                    timeout: Duration::ZERO,
                    ..Limits::default()
                },
                &()
            )
            .await,
        Err("turn_inventory_timeout")
    ));
    let exact = fixture
        .observe(
            Limits {
                rows: 1,
                page_size: 1,
                pages: 2,
                ..Limits::default()
            },
            &(),
        )
        .await
        .unwrap();
    assert_eq!(exact.selected, 1);
    assert!(fixture
        .observe(
            Limits::default(),
            &Mutate {
                database: fixture.database.clone(),
                replace_collection: true
            }
        )
        .await
        .is_err());
    fixture.assert_reads();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn inventory_refuses_malformed_settled_rows_and_preserves_unknown_agents() {
    let fixture = Fixture::new().await;
    let agent = agent("a");
    let mut record = saved(&agent);
    fixture.insert(&agent, &record).await;
    fixture
        .database
        .collection::<Document>("agents")
        .delete_many(doc! {})
        .await
        .unwrap();
    let report = fixture.observe(Limits::default(), &()).await.unwrap();
    assert_eq!(report.outcomes["agent_missing"], 1);
    record.reservation.settled = true;
    let mut malformed = document(&record);
    malformed.insert("document_schema_version", 999_i64);
    fixture
        .budget()
        .replace_one(doc! {"_id": "turn:a:2"}, malformed)
        .await
        .unwrap();
    assert!(matches!(
        fixture.observe(Limits::default(), &()).await,
        Err(INVALID)
    ));
    fixture.assert_reads();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI and CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn inventory_read_failure_never_becomes_an_empty_success() {
    let fixture = Fixture::new().await;
    let agent = agent("a");
    fixture.insert(&agent, &saved(&agent)).await;
    let second = tests::agent("b");
    fixture.insert(&second, &saved(&second)).await;
    let failpoints =
        Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    failpoints
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand",
        "mode": {"times": 1}, "data": {"failCommands": ["find"], "appName": &fixture.app_name,
        "errorCode": 11600}})
        .await
        .unwrap();
    let result = fixture.observe(Limits::default(), &()).await;
    failpoints
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(READ)));
    let late = fixture
        .observe(
            Limits {
                page_size: 1,
                ..Limits::default()
            },
            &FailLater {
                client: failpoints.clone(),
                app_name: fixture.app_name.clone(),
            },
        )
        .await;
    failpoints
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(late, Err(READ)));
    fixture.assert_reads();
    fixture.cleanup().await;
}

struct FailLater {
    client: Client,
    app_name: String,
}
#[async_trait::async_trait]
impl PageHook for FailLater {
    async fn after_page(&self, page: usize) -> Result<()> {
        if page == 1 {
            self.client
                .database("admin")
                .run_command(doc! {"configureFailPoint": "failCommand",
                "mode": {"times": 1}, "data": {"failCommands": ["find"], "appName": &self.app_name,
                "errorCode": 11600}})
                .await
                .unwrap();
        }
        Ok(())
    }
}

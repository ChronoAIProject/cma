//! Shared preflight budgets and heterogeneous historical identities.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, oid::ObjectId, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn billing_preflight_pages_are_shared_before_any_usage_erasure() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("sandbox_billing_state") {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_billing_pages_{}",
        uuid::Uuid::new_v4().simple()
    ));
    db.collection::<Document>("sandbox_billing_state")
        .insert_many((0..501).map(|n| doc! {
            "_id":format!("billing-{n:04}"),"owner_id":if n == 500 {"b"} else {"survivor"},
            "pending":if n == 500 {mongodb::bson::Bson::String("claim".into())} else {mongodb::bson::Bson::Null},
            "windows":[]
        })).await.unwrap();
    put(&db, "sandbox_usage", doc! {"_id":"usage-a","owner":"a"}).await;
    put(&db, "sandbox_usage", doc! {"_id":"usage-b","owner":"b"}).await;
    let inventory = ErasureInventory::default();
    let (a, b) = (SubjectId::new("a"), SubjectId::new("b"));
    let targets = [
        Target {
            subject: &a,
            inventory: &inventory,
        },
        Target {
            subject: &b,
            inventory: &inventory,
        },
    ];
    // Exercise the shared page budget independently of the new ownership
    // discovery, which can identify the same pending orphan before page one.
    let contexts: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|subject| {
            let mut context = context::Context::default();
            context.subject = subject.into();
            context
        })
        .collect();
    let mut cursor = Cursor::default();
    let error = guards::billing_page(&db, &contexts, &mut cursor)
        .await
        .unwrap_err();
    assert!(!cursor.billing_checked);
    assert!(matches!(&error,Error::Blocked(block)
        if block.reason==ErasureBlock::BillingSettlementPending
        && block.subject_hash==layer2_core::document_artifacts::digest(b"b")));
    assert_eq!(count(&db, "sandbox_usage").await, 2);
    {
        let commands = commands.lock().unwrap();
        assert_eq!(
            commands.len(),
            2,
            "one indexed refusal probe per cohort member"
        );
        assert!(commands.iter().all(|command| command
            .get_document("filter")
            .unwrap()
            .contains_key("owner_id")
            && command
                .get_i64("limit")
                .or_else(|_| command.get_i32("limit").map(i64::from))
                .unwrap()
                == 1));
    }
    assert!(
        matches!(erase_records(&db,&targets,&mut Cursor::default(),NOW).await,
        Err(Error::Blocked(block)) if block.reason==ErasureBlock::BillingSettlementPending
        && block.subject_hash==layer2_core::document_artifacts::digest(b"b"))
    );
    assert_eq!(count(&db, "sandbox_usage").await, 2);
    assert_eq!(count(&db, "sandbox_billing_state").await, 501);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_pagination_preserves_mixed_bson_identity_order() {
    let db = database().await;
    db.collection::<Document>("activity")
        .insert_many((0..500).map(|n| doc! {"_id":format!("string-{n:04}")}))
        .await
        .unwrap();
    let object = ObjectId::new();
    put(&db, "activity", doc! {"_id":object}).await;
    let first = super::scan::page(&db, "activity", None).await.unwrap();
    assert_eq!(first.len(), 500);
    let second = super::scan::page(&db, "activity", first.last().unwrap().get("_id"))
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].get_object_id("_id").unwrap(), object);
    assert!(super::scan::page(&db, "activity", second[0].get("_id"))
        .await
        .unwrap()
        .is_empty());
    db.drop().await.unwrap();
}

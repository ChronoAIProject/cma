use super::*;
use crate::auth::account_standing::ErasingSnapshot;
use layer2_core::{Authorization, Caller};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn disabled_startup_reads_absent_schema_without_creating_it() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!("cma_acd_absent_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(MongoAccountDeletionStore::from_database(&db));
    let standing = Arc::new(ErasingSnapshot::new(store.clone()));
    let refresh = standing.start().await;
    let owner = SubjectId::new("owner");
    assert!(!standing.erasing(&owner).await.unwrap());
    let service = AccountDeletionService::new(store.clone(), standing, false);
    let caller = Caller::new(owner.clone(), Authorization::default());
    let authority = OwnerSessionAuthority::verified_personal(owner);
    assert_eq!(
        service
            .request(&caller, Some(&authority), "new", true)
            .await,
        Err(AccountDeletionError::Unavailable)
    );
    assert_eq!(
        service.status(&caller, Some(&authority)).await,
        Err(AccountDeletionError::NotFound)
    );
    assert!(store.require_admission_schema().await.is_err());
    refresh.abort();
    assert!(db.list_collection_names().await.unwrap().is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn disabled_restart_retains_receipts_and_standing_without_repairing_schema() {
    let (_, db, writer) = fixture().await;
    let owner = SubjectId::new("owner");
    let accepted = writer.request(&owner, "accepted", NOW).await.unwrap();
    let before = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":accepted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    db.collection::<Document>(RECORDS)
        .drop_index("account_deletions_due")
        .await
        .unwrap();
    let store = Arc::new(MongoAccountDeletionStore::from_database(&db));
    let standing = Arc::new(ErasingSnapshot::new(store.clone()));
    let refresh = standing.start().await;
    assert!(standing.erasing(&owner).await.unwrap());
    let service = AccountDeletionService::new(store.clone(), standing, false);
    let caller = Caller::new(owner.clone(), Authorization::default());
    let authority = OwnerSessionAuthority::verified_personal(owner);
    assert_eq!(
        service.status(&caller, Some(&authority)).await.unwrap(),
        accepted.receipt()
    );
    assert_eq!(
        service
            .request(&caller, Some(&authority), "accepted", true)
            .await
            .unwrap(),
        accepted.receipt()
    );
    assert_eq!(
        service
            .request(&caller, Some(&authority), "new", true)
            .await,
        Err(AccountDeletionError::Unavailable)
    );
    assert!(store.require_admission_schema().await.is_err());
    refresh.abort();
    let after = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":accepted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert!(!db
        .collection::<Document>(RECORDS)
        .list_index_names()
        .await
        .unwrap()
        .iter()
        .any(|name| name == "account_deletions_due"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_requires_prepared_exact_schema() {
    let (_, db, store) = fixture().await;
    store.require_admission_schema().await.unwrap();
    db.collection::<Document>(RECORDS)
        .drop_index("account_deletions_active_subject")
        .await
        .unwrap();
    db.collection::<Document>(RECORDS)
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"active_subject_hash":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("account_deletions_active_subject".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    assert!(store.require_admission_schema().await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn standing_status_read_is_causally_after_a_unique_primary_barrier() {
    use mongodb::{
        event::command::CommandEvent,
        event::EventHandler,
        options::{ClientOptions, ReadPreference, SelectionCriteria},
        Client,
    };
    use std::sync::Mutex;
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let times = Arc::new(Mutex::new(Vec::new()));
    let completed = times.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.selection_criteria = Some(SelectionCriteria::ReadPreference(
        ReadPreference::SecondaryPreferred {
            options: Default::default(),
        },
    ));
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(event) if event.command_name == "find" => {
            observed.lock().unwrap().push(event.command);
        }
        CommandEvent::Succeeded(event) if event.command_name == "find" => {
            completed
                .lock()
                .unwrap()
                .push(event.reply.get_timestamp("operationTime").unwrap());
        }
        _ => {}
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!(
        "cma_acd_barrier_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let writer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let acceptance = MongoAccountDeletionStore::open(&writer.database(db.name()))
        .await
        .unwrap();
    let owner = SubjectId::new("owner");
    acceptance.request(&owner, "accepted", NOW).await.unwrap();
    let store = MongoAccountDeletionStore::from_database(&db);
    commands.lock().unwrap().clear();
    assert!(store.erasing(&owner).await.unwrap());
    let reads = commands.lock().unwrap().clone();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0].get_str("find").unwrap(), "sandbox_admissions");
    assert_eq!(
        reads[0].get_document("filter").unwrap(),
        &doc! {"_id":owner.as_str()}
    );
    let barrier = reads[0].get_document("readConcern").unwrap();
    assert_eq!(barrier.get_str("level").unwrap(), "linearizable");
    assert!(!barrier.contains_key("afterClusterTime"));
    assert_eq!(reads[1].get_str("find").unwrap(), RECORDS);
    let status = reads[1].get_document("readConcern").unwrap();
    assert_eq!(status.get_str("level").unwrap(), "majority");
    assert_eq!(
        status.get_timestamp("afterClusterTime").unwrap(),
        times.lock().unwrap()[0]
    );
    assert_eq!(reads[0].get("lsid"), reads[1].get("lsid"));
    for read in reads {
        assert_eq!(read.get_i32("maxTimeMS").unwrap(), 750);
        assert!(!read.contains_key("txnNumber"));
    }
    db.drop().await.unwrap();
}

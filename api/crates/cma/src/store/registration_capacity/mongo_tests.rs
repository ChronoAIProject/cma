use super::*;
use layer2_core::admission::AdmissionCeilings;
use std::sync::Arc;

async fn fixture(activated: bool) -> (Client, String, MongoRegistrationCapacityStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!(
        "cma_registration_capacity_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = MongoRegistrationCapacityStore::open(&client, &name, activated)
        .await
        .unwrap();
    (client, name, store)
}

async fn set_limit(store: &MongoRegistrationCapacityStore, limit: Option<u32>) {
    crate::store::settings_barrier::test_support::set_ceilings(
        &store.client,
        store.database.name(),
        AdmissionCeilings {
            registrations: limit,
            ..Default::default()
        },
    )
    .await;
}

async fn count(store: &MongoRegistrationCapacityStore) -> u64 {
    let row = store
        .admissions
        .find_one(capacity_key())
        .await
        .unwrap()
        .unwrap();
    read_count(&row).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn registration_quarantine_keeps_existing_subject_and_preserves_bad_marker() {
    let (client, name, store) = fixture(true).await;
    let existing = SubjectId::new("already-registered");
    assert!(store.admit(&existing).await.unwrap());
    store
        .admissions
        .update_one(capacity_key(), doc! {"$set": {"count": "invalid"}})
        .await
        .unwrap();
    let before = store
        .admissions
        .find_one(capacity_key())
        .await
        .unwrap()
        .unwrap();

    for writes_enabled in [false, true] {
        let restarted = MongoRegistrationCapacityStore::open(&client, &name, writes_enabled)
            .await
            .unwrap();
        assert!(restarted.activation_observation().load(Ordering::Relaxed));
        assert!(restarted.admit(&existing).await.unwrap());
        assert!(!restarted.available().await.unwrap());
        assert!(!restarted
            .admit(&SubjectId::new("new-subject"))
            .await
            .unwrap());
        assert_eq!(
            store.admissions.find_one(capacity_key()).await.unwrap(),
            Some(before.clone())
        );
    }
    assert_eq!(
        store
            .admissions
            .count_documents(doc! {"_id.scope": "registration"})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_registration_boundary_admits_exactly_the_available_capacity() {
    let (client, name, first) = fixture(true).await;
    set_limit(&first, Some(7)).await;
    let second = MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    let first = Arc::new(first);
    let second = Arc::new(second);
    let barrier = Arc::new(tokio::sync::Barrier::new(50));
    let results = futures_util::future::join_all((0..50).map(|n| {
        let store = if n % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let barrier = barrier.clone();
        async move {
            barrier.wait().await;
            store
                .admit(&SubjectId::new(format!("public-{n}")))
                .await
                .unwrap()
        }
    }))
    .await;
    assert_eq!(results.iter().filter(|accepted| **accepted).count(), 7);
    assert_eq!(count(&first).await, 7);
    assert_eq!(
        first
            .admissions
            .count_documents(doc! {"_id.scope":"registration"})
            .await
            .unwrap(),
        7
    );
    assert!(!first.available().await.unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn duplicate_callbacks_count_once_and_replay_after_lowering_to_zero() {
    let (client, name, store) = fixture(true).await;
    set_limit(&store, Some(1)).await;
    let store = Arc::new(store);
    let subject = SubjectId::new("one-public-subject");
    let results = futures_util::future::join_all((0..24).map(|_| {
        let store = store.clone();
        let subject = subject.clone();
        async move { store.admit(&subject).await.unwrap() }
    }))
    .await;
    assert!(results.into_iter().all(|accepted| accepted));
    assert_eq!(count(&store).await, 1);
    set_limit(&store, Some(0)).await;
    assert!(store.existing_subject(&subject).await.unwrap());
    assert!(store.admit(&subject).await.unwrap());
    assert!(!store
        .admit(&SubjectId::new("another-subject"))
        .await
        .unwrap());
    assert_eq!(count(&store).await, 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_sign_in_directory_and_owned_resources_are_never_public() {
    let (client, name, store) = fixture(true).await;
    set_limit(&store, Some(0)).await;
    for (collection, field) in EXISTING_EVIDENCE {
        let subject = SubjectId::new(format!("existing-{collection}"));
        let mut row = doc! {"_id":subject.as_str()};
        if *field != "_id" {
            if *field == "record.owner" {
                row.insert("record", doc! {"owner":subject.as_str()});
            } else {
                row.insert(*field, subject.as_str());
            }
        }
        store
            .database
            .collection::<Document>(collection)
            .insert_one(row)
            .await
            .unwrap();
        assert!(
            store.existing_subject(&subject).await.unwrap(),
            "{collection}"
        );
        assert!(store.admit(&subject).await.unwrap(), "{collection}");
        assert!(!is_public(
            &store.admissions,
            &subject,
            Some(&mut client.start_session().await.unwrap())
        )
        .await
        .unwrap());
    }
    assert_eq!(count(&store).await, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn never_opened_gate_writes_no_cohort_records_and_activation_survives_closure() {
    let (client, name, store) = fixture(false).await;
    assert!(store
        .admit(&SubjectId::new("before-activation"))
        .await
        .unwrap());
    assert_eq!(store.admissions.count_documents(doc! {}).await.unwrap(), 0);
    let enabled = MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    assert!(enabled
        .admit(&SubjectId::new("after-activation"))
        .await
        .unwrap());
    let closed = MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    assert!(closed
        .admit(&SubjectId::new("after-closure"))
        .await
        .unwrap());
    assert_eq!(count(&closed).await, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unreadable_ceilings_refuse_fresh_registration_but_keep_existing_subjects() {
    let (client, name, store) = fixture(true).await;
    store
        .settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_ceilings": false}},
        )
        .await
        .unwrap();
    store
        .database
        .collection::<Document>("auth_subjects")
        .insert_one(doc! {"_id":"legacy-owner"})
        .await
        .unwrap();
    assert!(!store.admit(&SubjectId::new("new-owner")).await.unwrap());
    assert!(!store.available().await.unwrap());
    assert!(store.admit(&SubjectId::new("legacy-owner")).await.unwrap());
    assert_eq!(count(&store).await, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn a_receipt_write_failure_rolls_back_the_capacity_increment() {
    let (client, name, store) = fixture(true).await;
    set_limit(&store, Some(1)).await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod":"sandbox_admissions", "validator":{"_id.scope":{"$ne":"registration"}},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    assert!(store
        .admit(&SubjectId::new("receipt-failure"))
        .await
        .is_err());
    assert_eq!(count(&store).await, 0);
    assert!(store.available().await.unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unset_registration_advisory_adds_no_capacity_query() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    use std::sync::Mutex;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if let Some(Bson::String(collection)) = event.command.get(&event.command_name) {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, collection.clone()));
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_registration_unset_{}", uuid::Uuid::new_v4().simple());
    let store = MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    assert!(store.available().await.unwrap());
    assert_eq!(
        *trace.lock().unwrap(),
        vec![("find".into(), "platform_settings".into())]
    );
    trace.lock().unwrap().clear();
    assert!(store.available_with_limit(Ok(None)).await.unwrap());
    assert!(trace.lock().unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

mod cutoff;

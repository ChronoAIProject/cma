use layer2_core::admission::{AdmissionControl, AdmissionControlStore};
use layer2_core::{PlatformSettingsStore, StoreError, SubjectId};
use mongodb::bson::{doc, Document};
use mongodb::Client;

use super::MongoAdmissionControlStore;

async fn fixture() -> (Client, String, MongoAdmissionControlStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_admission_control_{}", uuid::Uuid::new_v4().simple());
    let store = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    (client, name, store)
}

async fn audit_count(client: &Client, name: &str) -> u64 {
    client
        .database(name)
        .collection::<Document>("activity")
        .count_documents(doc! {"record.target.id": "admission_control"})
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn control_is_shared_and_preserves_the_independent_settings_revision() {
    let (client, name, first) = fixture().await;
    let second = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let settings = crate::store::settings::MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    let initial = settings.read().await.unwrap();
    let actor = SubjectId::new("operator-a");
    assert_eq!(first.read().await.unwrap(), AdmissionControl::default());
    let stop = first
        .change(0, &actor, "contain incident", true, 100)
        .await
        .unwrap();
    assert_eq!(second.read().await.unwrap(), stop);
    assert_eq!(settings.read().await.unwrap(), initial);
    let mut edited = initial.settings.clone();
    edited.limits.visitor_max_launches_per_day += 1;
    let saved_settings = settings.replace(initial.revision, &edited).await.unwrap();
    assert_eq!(saved_settings.revision, initial.revision + 1);
    assert_eq!(first.read().await.unwrap(), stop);
    let resume = second
        .change(1, &actor, "resolved", false, 101)
        .await
        .unwrap();
    assert_eq!(first.read().await.unwrap(), resume);
    assert_eq!(settings.read().await.unwrap(), saved_settings);
    assert_eq!(audit_count(&client, &name).await, 2);
    assert_eq!(first.read_with_history().await.unwrap().history.len(), 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn simultaneous_stops_are_first_writer_wins_with_one_audit_row() {
    let (client, name, first) = fixture().await;
    let second = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let a = SubjectId::new("operator-a");
    let b = SubjectId::new("operator-b");
    let (left, right) = tokio::join!(
        first.change(0, &a, "first operator", true, 100),
        second.change(0, &b, "second operator", true, 101),
    );
    let left = left.unwrap();
    assert_eq!(right.unwrap(), left);
    assert_eq!(left.revision, 1);
    assert_eq!(audit_count(&client, &name).await, 1);
    assert_eq!(
        first.read_with_history().await.unwrap().history,
        vec![left.last_change.unwrap()]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_resume_racing_a_new_stop_never_clears_it() {
    let (client, name, first) = fixture().await;
    let second = MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    let actor = SubjectId::new("operator-a");
    first
        .change(0, &actor, "first incident", true, 100)
        .await
        .unwrap();
    first
        .change(1, &actor, "resolved", false, 101)
        .await
        .unwrap();
    let (stop, resume) = tokio::join!(
        first.change(2, &actor, "new incident", true, 102),
        second.change(1, &actor, "stale resolve", false, 103),
    );
    let stopped = stop.unwrap();
    assert!(matches!(
        resume,
        Err(StoreError::RevisionConflict { expected: 1, .. })
    ));
    assert_eq!(second.read().await.unwrap(), stopped);
    assert_eq!(audit_count(&client, &name).await, 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_write_failure_aborts_the_control_change() {
    let (client, name, store) = fixture().await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod": "activity", "validator": {"audit_failure_fixture": true},
            "validationLevel": "strict", "validationAction": "error",
        })
        .await
        .unwrap();
    assert!(store
        .change(0, &SubjectId::new("operator-a"), "incident", true, 100)
        .await
        .is_err());
    assert_eq!(store.read().await.unwrap(), AdmissionControl::default());
    assert_eq!(audit_count(&client, &name).await, 0);
    assert!(store.read_with_history().await.unwrap().history.is_empty());
    client.database(&name).drop().await.unwrap();
}

#[path = "history_tests.rs"]
mod history_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn audit_rows_retain_actor_time_revision_and_control_vocabulary() {
    let (client, name, store) = fixture().await;
    let actor = SubjectId::new("operator-a");
    let stopped = store
        .change(0, &actor, "contain incident", true, 100)
        .await
        .unwrap();
    let row = client
        .database(&name)
        .collection::<Document>("activity")
        .find_one(doc! {"record.target.id": "admission_control"})
        .await
        .unwrap()
        .unwrap();
    let record: layer2_core::ActivityRecord =
        mongodb::bson::deserialize_from_document(row.get_document("record").unwrap().clone())
            .unwrap();
    assert_eq!(record.actor, Some(actor.clone()));
    assert_eq!(record.at, "1970-01-01T00:01:40Z");
    assert_eq!(record.action, layer2_core::ActivityAction::Configure);
    assert_eq!(
        record.detail.get(layer2_core::DetailKey::Revision),
        Some("1")
    );
    assert_eq!(
        record.detail.get(layer2_core::DetailKey::Status),
        Some("stopped")
    );
    let change = stopped.last_change.unwrap();
    assert_eq!(change.actor, actor);
    assert_eq!(change.reason, "contain incident");
    assert!(change.old_stop.is_none());
    assert_eq!(change.new_stop, stopped.operator_stop);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn malformed_control_is_refused_instead_of_becoming_open() {
    let (client, name, store) = fixture().await;
    let settings = client
        .database(&name)
        .collection::<Document>("platform_settings");
    for invalid in [
        mongodb::bson::Bson::Null,
        doc! {"revision": -1_i64}.into(),
        doc! {"revision": "1"}.into(),
    ] {
        settings
            .update_one(
                doc! {"_id": "platform"},
                doc! {"$set": {
                    "document_schema_version": 1_i64, "admission_control": invalid,
                }},
            )
            .upsert(true)
            .await
            .unwrap();
        assert!(matches!(store.read().await, Err(StoreError::Malformed(_))));
        assert!(matches!(
            store
                .change(0, &SubjectId::new("operator-a"), "incident", true, 100)
                .await,
            Err(StoreError::Malformed(_))
        ));
    }
    client.database(&name).drop().await.unwrap();
}

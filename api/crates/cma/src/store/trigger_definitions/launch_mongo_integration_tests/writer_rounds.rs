//! Test-only process boundary for executing each revision's real Mongo writers.
use super::*;
use crate::store::writer_fixture::*;
use futures_util::TryStreamExt;
use layer2_core::PlatformSettingsStore;
use mongodb::Client;
use serde_json::{json, Value};

pub(super) async fn open(client: &Client, database: &str) -> MongoTriggerDefinitionStore {
    let profiles = Arc::new(
        crate::store::sandbox_mongo::MongoProfileStore::open(client, database)
            .await
            .unwrap(),
    );
    MongoTriggerDefinitionStore::open(client, database, profiles)
        .await
        .unwrap()
}

pub(super) async fn snapshot(client: &Client, database: &str) -> Value {
    let db = client.database(database);
    let row = db
        .collection::<Document>("trigger_launches")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let id = row.get_str("_id").unwrap();
    let fence = db
        .collection::<Document>("sandbox_admissions")
        .find_one(doc! {"_id.scope":"global_sandbox_capacity"})
        .await
        .unwrap();
    let reservation = db
        .collection::<Document>("sandbox_admissions")
        .find_one(doc! {"_id.scope":"launch_home_capacity","_id.launch":id})
        .await
        .unwrap();
    let events: Vec<Document> = db
        .collection::<Document>("trigger_launch_events")
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    json!({"database":database,"id_hash":hash(&id),
        "revision":row.get_i64("revision").unwrap(),
        "mutation_version":row.get_i64("mutation_version").unwrap(),
        "marker":row.get("home_capacity_admission"),
        "state":row.get("state"),"lease_fence":row.get("work_lease_fence"),
        "pending_homes":fence.as_ref().and_then(|d| d.get("pending_launch_homes")),
        "reservation_state":reservation.as_ref().and_then(|d| d.get("state")),
        "event_count":events.len(),"event_hash":hash(&events),"row_hash":hash(&row)})
}

pub(super) async fn seed() -> Seeded {
    let case = input("CMA_WRITER_CASE");
    assert!(case.len() <= 20 && case.bytes().all(|b| b.is_ascii_alphanumeric()));
    // Validate the endpoint before the existing fixture creates any collection.
    let _client = client().await;
    let seed = seeded(&format!("writer_{case}")).await;
    admit_started(&seed).await;
    seed
}

pub(super) async fn admit_started(seed: &Seeded) {
    let trigger = published(seed, "writer_round").await;
    let mut candidate = admit_candidate(seed, &trigger, launch(&trigger, 'a', 'b')).await;
    let expected_revision = candidate.revision;
    candidate
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: vec![],
            confirmed_at: NOW.into(),
        })
        .unwrap();
    seed.store
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: candidate,
            event_at: NOW.into(),
            events: vec![
                LaunchEventKind::ConfirmationRecorded,
                LaunchEventKind::StateChanged {
                    from: LaunchState::Draft,
                    to: LaunchState::Resolving,
                },
            ],
        })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn launch_round() {
    let action = input("CMA_WRITER_ACTION");
    if action == "seed" {
        let seed = seed().await;
        emit(snapshot(&seed.client, &seed.database).await);
        return;
    }
    let client = client().await;
    let database = database_name();
    let store = open(&client, &database).await;
    let row = client
        .database(&database)
        .collection::<Document>("trigger_launches")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let id = LaunchId(row.get_str("_id").unwrap().into());
    let owner = SubjectId::new(row.get_str("owner").unwrap());
    let before = snapshot(&client, &database).await;
    let Some(mut launch) = store.launch(&id, &owner).await.unwrap() else {
        panic!("accepted Launch must remain readable");
    };
    let mut stale_rejected = None;
    match action.as_str() {
        "read" => {}
        "claim" | "contend" => {
            let now = input("CMA_WRITER_NOW");
            let until = input("CMA_WRITER_UNTIL");
            let result = store
                .claim_launch(&id, "writer-fixture", now.clone(), until.clone())
                .await
                .unwrap();
            let Some(claimed) = result else {
                assert_eq!(action, "contend", "due Launch must be claimed");
                emit(json!({"action":action,"claimed":false,"before":before,
                    "after":snapshot(&client,&database).await}));
                return;
            };
            assert_eq!(claimed.revision, launch.revision + 1);
            assert_eq!(claimed.id, launch.id);
            assert!(store
                .claim_launch(&id, "other-fixture", now, until)
                .await
                .unwrap()
                .is_none());
            let stale = LaunchCommit {
                expected_revision: launch.revision,
                launch: claimed,
                event_at: LATER.into(),
                events: vec![],
            };
            stale_rejected = Some(matches!(
                store.commit_launch(stale).await,
                Err(LaunchStoreError::Conflict { .. })
            ));
            assert_eq!(stale_rejected, Some(true));
        }
        "cancel" => {
            let expected_revision = launch.revision;
            let from = launch.state;
            let now = input("CMA_WRITER_NOW");
            launch.cancel(now.clone()).unwrap();
            let commit = LaunchCommit {
                expected_revision,
                launch,
                event_at: now,
                events: vec![
                    LaunchEventKind::StateChanged {
                        from,
                        to: LaunchState::Cancelled,
                    },
                    LaunchEventKind::Cancelled,
                ],
            };
            let completed = store.commit_launch(commit.clone()).await.unwrap();
            assert_eq!(completed.state, LaunchState::Cancelled);
            stale_rejected = Some(matches!(
                store.commit_launch(commit).await,
                Err(LaunchStoreError::Conflict { .. })
            ));
            assert_eq!(stale_rejected, Some(true));
        }
        _ => panic!("unsupported writer action"),
    }
    let after = snapshot(&client, &database).await;
    assert_eq!(before["id_hash"], after["id_hash"]);
    if action == "read" {
        assert_eq!(before, after);
    }
    emit(json!({"action":action,"before":before,"after":after,
        "stale_revision_rejected":stale_rejected,"claimed":action=="claim" || action=="contend"}));
}

pub(super) async fn policy_snapshot(client: &Client, database: &str) -> Value {
    let db = client.database(database);
    let settings = db.collection::<Document>("platform_settings");
    let row = settings.find_one(doc! {"_id":"platform"}).await.unwrap();
    let history: Vec<Document> = settings
        .find(doc! {"_id":{"$regex":"^admission_ceilings_change:"}})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let activity: Vec<Document> = db
        .collection::<Document>("activity")
        .find(doc! {"record.target.id":"admission_ceilings"})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let activation = db
        .collection::<Document>("sandbox_admissions")
        .find_one(doc! {"_id.scope":"registration_capacity"})
        .await
        .unwrap();
    json!({"policy":row.as_ref().and_then(|r|r.get("admission_ceilings")),
        "history_hash":hash(&history),"history_count":history.len(),
        "activity_hash":hash(&activity),"activity_count":activity.len(),
        "activation_hash":hash(&activation)})
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn settings_round() {
    let client = client().await;
    let database = database_name();
    let store = crate::store::MongoPlatformSettingsStore::open(&client, &database)
        .await
        .unwrap();
    let before = policy_snapshot(&client, &database).await;
    let previous = store.read().await.unwrap();
    let mut settings = previous.settings.clone();
    settings.inactivity.pause_after_seconds = if settings.inactivity.pause_after_seconds == 1200 {
        1800
    } else {
        1200
    };
    let changed = store.replace(previous.revision, &settings).await.unwrap();
    assert_eq!(changed.revision, previous.revision + 1);
    assert_eq!(changed.settings, settings);
    assert!(matches!(
        store.replace(previous.revision, &settings).await,
        Err(StoreError::RevisionConflict { .. })
    ));
    let after = policy_snapshot(&client, &database).await;
    assert_eq!(
        before, after,
        "settings CAS must preserve independent retained policy/history"
    );
    emit(
        json!({"database":database,"revision_before":previous.revision,
        "revision_after":changed.revision,"retained":after,"stale_revision_rejected":true}),
    );
}

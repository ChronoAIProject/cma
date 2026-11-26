//! Older snapshot transactions must drain before the reference graph is read.
use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use layer2_core::profiles::{AgentProfileStore, ProfileStoreError, WorkspaceProfileStore};
use layer2_core::WorkspaceProfileId;
use mongodb::{
    bson::{doc, DateTime, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const NOW: &str = "2026-11-14T00:00:00Z";

#[derive(Clone, Default)]
struct Commands(Arc<Mutex<Vec<(String, Document)>>>);

impl Commands {
    async fn database(&self, name: &str) -> Database {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let observed = self.0.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        Client::with_options(options).unwrap().database(name)
    }

    fn take(&self) -> Vec<(String, Document)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    fn assert_only_barrier_writes(&self) {
        for (name, command) in self.take() {
            assert!(
                !matches!(name.as_str(), "find" | "aggregate"),
                "reference discovery ran before the drain deadline: {command:?}"
            );
            assert!(
                !matches!(name.as_str(), "insert" | "delete" | "findAndModify"),
                "an erasure effect ran before the drain deadline: {command:?}"
            );
            if name == "update" {
                assert_eq!(
                    command
                        .get_document("writeConcern")
                        .unwrap()
                        .get_str("w")
                        .ok(),
                    Some("majority")
                );
                assert!([
                    "agent_profiles",
                    "workspace_profiles",
                    "sandbox_profiles",
                    "trigger_definitions",
                    "trigger_definition_revisions"
                ]
                .contains(&command.get_str("update").unwrap()));
                for update in command.get_array("updates").unwrap() {
                    let mutation = update.as_document().unwrap().get_document("u").unwrap();
                    assert_eq!(mutation.len(), 2);
                    assert_eq!(
                        mutation.get_document("$set").unwrap(),
                        &doc! {"account_erasure_fenced":true}
                    );
                    assert_eq!(
                        mutation.get_document("$currentDate").unwrap(),
                        &doc! {"account_erasure_fenced_at":true}
                    );
                }
            }
        }
    }
}

fn options() -> Options {
    Options {
        allocation_rollups_enabled: Some(false),
        agent_shared_observation: Some(false),
        retain_legal_consents: false,
        drain_qualification: Some(TransactionDrainQualification::fixture()),
        billing_enabled: false,
        transaction_drain_minimum_secs: 120,
    }
}

fn assert_waiting(batch: &Batch, cursor: &Cursor) {
    assert!(batch.waiting && batch.has_more);
    assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
    assert!(batch.discovered.is_empty() && batch.pending_key_ids.is_empty());
    assert!(cursor.contexts.is_empty());
    assert!(cursor.retained_profiles.is_none());
    assert!(!cursor.drain_complete);
}

async fn wait_for_server_deadline(db: &Database, deadline: DateTime) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let clock = db.run_command(doc! {"hello":1}).await.unwrap();
            if *clock.get_datetime("localTime").unwrap() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the one-second test drain must expire promptly");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn barrier_deadline_survives_reload_without_reference_discovery_or_erasure() {
    let seed = seeded("round2_barrier_reload").await;
    let independent = seed.client.database(&seed.database);
    let collection = independent.collection::<Document>("agent_profiles");
    let original = collection
        .find_one(doc! {"_id":seed.target.agent_profile.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let commands = Commands::default();
    let db = commands.database(&seed.database).await;
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &seed.owner,
        inventory: &inventory,
    }];
    let mut cursor = Cursor {
        test_drain_seconds: Some(30),
        ..Default::default()
    };
    let batch = erase_records_with_options(&db, &targets, &mut cursor, NOW, options())
        .await
        .unwrap();
    assert_waiting(&batch, &cursor);
    commands.assert_only_barrier_writes();
    let deadline = cursor.drain_until.unwrap();
    let fenced = collection
        .find_one(doc! {"_id":seed.target.agent_profile.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fenced.get("record"), original.get("record"));
    assert_eq!(fenced.get_bool("account_erasure_fenced").ok(), Some(true));
    assert!(*fenced.get_datetime("account_erasure_fenced_at").unwrap() < deadline);

    let serialized = serde_json::to_vec(&cursor).unwrap();
    let mut reloaded: Cursor = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(reloaded.drain_until, Some(deadline));
    assert!(reloaded.test_drain_seconds.is_none());
    let batch = erase_records_with_options(&db, &targets, &mut reloaded, NOW, options())
        .await
        .unwrap();
    assert_waiting(&batch, &reloaded);
    assert_eq!(reloaded.drain_until, Some(deadline));
    commands.assert_only_barrier_writes();
    assert_eq!(
        collection
            .find_one(doc! {"_id":seed.target.agent_profile.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        fenced
    );
    independent.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn barrier_retains_a_visitor_pin_committed_by_a_transaction_from_before_the_fence() {
    let seed = seeded("round2_barrier_old_transaction").await;
    let independent = seed.client.database(&seed.database);
    // Install collection/index metadata before the snapshot transaction starts.
    super::indexes::ensure(&independent).await.unwrap();
    let original = seed
        .profiles
        .agent_profile_revision(&seed.target.agent_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let revisions = independent.collection::<Document>("agent_profile_revisions");
    let original_document = revisions
        .find_one(doc! {"profile_id":original.profile_id.as_str(),"revision":1_i64})
        .await
        .unwrap()
        .unwrap();
    let mut unreferenced = original_document.clone();
    unreferenced.insert(
        "_id",
        format!("{}:0000000002", original.profile_id.as_str()),
    );
    unreferenced.insert("revision", 2_i64);
    unreferenced
        .get_document_mut("record")
        .unwrap()
        .insert("revision", 2_i64);
    revisions.insert_one(unreferenced).await.unwrap();
    let mut new_pin = seed
        .profiles
        .workspace_profile(&seed.target.workspace_profile.id)
        .await
        .unwrap()
        .unwrap();
    new_pin.id = WorkspaceProfileId("wkp_barrier_refused".into());
    new_pin.owner = SubjectId::new("visitor");
    let commands = Commands::default();
    let db = commands.database(&seed.database).await;
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let snapshot = db
        .collection::<Document>("agent_profiles")
        .find_one(doc! {"_id":original.profile_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    assert!(!snapshot.contains_key("account_erasure_fenced"));
    let _ = commands.take();

    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor {
        test_drain_seconds: Some(1),
        ..Default::default()
    };
    let waiting = erase_records_with_options(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
        options(),
    )
    .await
    .unwrap();
    assert_waiting(&waiting, &cursor);
    commands.assert_only_barrier_writes();
    assert_eq!(
        seed.profiles
            .create_workspace_profile(new_pin.clone())
            .await,
        Err(ProfileStoreError::ReferenceUnavailable)
    );
    assert!(seed
        .profiles
        .workspace_profile(&new_pin.id)
        .await
        .unwrap()
        .is_none());
    let old_snapshot = db
        .collection::<Document>("agent_profiles")
        .find_one(doc! {"_id":original.profile_id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_snapshot, snapshot);
    // The old transaction read the Profile before the fence. Its only write is
    // the visitor's new Agent; it must be allowed to commit that retained pin.
    let visitor = doc! {"_id":"agt_barrier_visitor","document_schema_version":1_i64,
    "owner":"visitor","record":{"profile":{"id":original.profile_id.as_str(),"revision":1_i64}}};
    db.collection::<Document>("agents")
        .insert_one(visitor.clone())
        .session(&mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
    let committed = commands.take();
    assert!(committed.iter().any(
        |(name, command)| name == "insert" && command.get_str("insert").ok() == Some("agents")
    ));
    assert!(committed.iter().all(|(name, _)| name != "update"));
    let deadline = cursor.drain_until.unwrap();
    // A different worker resumes the actual persisted deadline, rather than
    // using an in-process timer or resetting the transaction drain interval.
    cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
    assert_eq!(cursor.drain_until, Some(deadline));
    wait_for_server_deadline(&independent, deadline).await;
    let mut complete = false;
    for _ in 0..1000 {
        let batch = erase_records_with_options(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
            options(),
        )
        .await
        .unwrap();
        assert!(batch.pending_key_ids.is_empty());
        super::progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
        if !batch.has_more {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "erasure did not finish after the old transaction committed"
    );
    assert!(cursor.drain_complete);
    assert!(cursor
        .retained_profiles
        .as_ref()
        .unwrap()
        .contains(original_document.get_str("_id").unwrap()));
    assert_eq!(
        db.collection::<Document>("agents")
            .find_one(doc! {"_id":"agt_barrier_visitor"})
            .await
            .unwrap(),
        Some(visitor)
    );
    assert_eq!(
        seed.profiles
            .agent_profile_revision(&original.profile_id, 1)
            .await
            .unwrap(),
        Some(original.clone())
    );
    assert!(seed
        .profiles
        .agent_profile_revision(&original.profile_id, 2)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        seed.profiles
            .agent_profile(&original.profile_id)
            .await
            .unwrap()
            .unwrap()
            .owner
            .as_str(),
        layer2_core::DELETED_ACCOUNT_SUBJECT
    );
    assert!(commands
        .take()
        .iter()
        .any(|(name, command)| name == "find" && command.get_str("find").ok() == Some("agents")));
    independent.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_late_catalogue_insert_restarts_the_barrier_and_reference_snapshot() {
    let seed = seeded("round2_barrier_late_catalogue").await;
    let db = seed.client.database(&seed.database);
    let profiles = db.collection::<Document>("agent_profiles");
    let revisions = db.collection::<Document>("agent_profile_revisions");
    let mut profile = profiles.find_one(doc! {}).await.unwrap().unwrap();
    let mut revision = revisions.find_one(doc! {}).await.unwrap().unwrap();
    let id = "agp_late_catalogue";
    profile.insert("_id", id);
    profile.get_document_mut("record").unwrap().insert("id", id);
    revision.insert("_id", format!("{id}:0000000001"));
    revision.insert("profile_id", id);
    revision
        .get_document_mut("record")
        .unwrap()
        .insert("profile_id", id);
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor {
        test_drain_seconds: Some(0),
        ..Default::default()
    };
    for _ in 0..1000 {
        let batch = erase_records_with_options(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
            options(),
        )
        .await
        .unwrap();
        super::progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
        if cursor.retained_profiles.is_some() {
            break;
        }
    }
    assert!(cursor.drain_complete && cursor.retained_profiles.is_some());
    // A pre-gate nontransactional request can finish inserting an owned draft
    // after the first snapshot has drained. Never loop on an unfenced root or
    // apply the earlier reference graph to that newly introduced identity.
    profiles.insert_one(profile).await.unwrap();
    let mut unused = revision.clone();
    unused.insert("_id", format!("{id}:0000000002"));
    unused.insert("revision", 2_i64);
    unused
        .get_document_mut("record")
        .unwrap()
        .insert("revision", 2_i64);
    revisions.insert_many([revision, unused]).await.unwrap();
    db.collection::<Document>("agents").insert_one(doc! {"_id":"agt_late_visitor","owner":"visitor","record":{"profile":{"id":id,"revision":1_i64}}}).await.unwrap();
    cursor.operation = super::progress::operations()
        .iter()
        .position(|(entry, _)| entry.collection == "agent_profiles")
        .unwrap();
    cursor.after = None;
    super::progress::bind(&mut cursor, &super::progress::operations());
    cursor.test_drain_seconds = Some(1);
    let batch = erase_records_with_options(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
        options(),
    )
    .await
    .unwrap();
    assert!(batch.waiting && batch.has_more);
    assert!(!cursor.drain_complete);
    assert!(cursor.retained_profiles.is_none());
    assert!(revisions
        .find_one(doc! {"profile_id":id})
        .await
        .unwrap()
        .is_some());
    let batch = erase_records_with_options(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
        options(),
    )
    .await
    .unwrap();
    assert!(batch.waiting && batch.deleted.is_empty());
    assert!(profiles
        .find_one(doc! {"_id":id,"account_erasure_fenced":true})
        .await
        .unwrap()
        .is_some());
    wait_for_server_deadline(&db, cursor.drain_until.unwrap()).await;
    let mut complete = false;
    for _ in 0..1000 {
        let batch = erase_records_with_options(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
            options(),
        )
        .await
        .unwrap();
        super::progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
        if !batch.has_more {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert!(revisions
        .find_one(doc! {"profile_id":id})
        .await
        .unwrap()
        .is_some());
    let retained = profiles.find_one(doc! {"_id":id}).await.unwrap().unwrap();
    assert_eq!(
        retained.get_str("owner").unwrap(),
        layer2_core::DELETED_ACCOUNT_SUBJECT
    );
    assert_eq!(retained.get_str("visibility").unwrap(), "private");
    assert!(revisions
        .find_one(doc! {"profile_id":id,"revision":2_i64})
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}

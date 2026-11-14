use std::sync::{Arc, Mutex};

use layer2_core::sandbox::store::SandboxStore;
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use mongodb::options::{ClientOptions, ReadConcern, WriteConcern};
use mongodb::Client;

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};

fn context(store: &MongoSandboxStore) -> RuntimeActivitySourceContext {
    RuntimeActivitySourceContext {
        source_id: "mongo-activity-fixture".into(),
        adapter_version: "1".into(),
        policy_id: "test-only".into(),
        producer_id: "test".into(),
        database_id: store.sandboxes.namespace().db,
        api_image: "test".into(),
        source_revision: "test".into(),
        process_incarnation: layer2_core::sandbox::runtime_desktop_process_incarnation().into(),
        serving_processes: 1,
        replica_mode: false,
        clock_uncertainty_ms: 0,
        coverage: RuntimeActivityCoverage::MonotonicWritersV1,
    }
}

async fn observe(store: &MongoSandboxStore, id: &SandboxId) -> RuntimeActivitySnapshot {
    match store
        .observe_runtime_activity(id, &context(store))
        .await
        .unwrap()
    {
        RuntimeActivityObservation::Snapshot(snapshot) => *snapshot,
        RuntimeActivityObservation::Unknown(reason) => {
            panic!("fixture observation unavailable: {reason:?}")
        }
    }
}

fn fixture_proof(final_observation: RuntimeActivitySnapshot) -> RuntimeActivityProof {
    let mut initial = final_observation.clone();
    // Synthetic time is test-only. The final vector/watermark is the actual
    // snapshot; these tests prove comparison/fencing, not elapsed production quietness.
    initial.capture_started_at_ms =
        final_observation.capture_started_at_ms - RUNTIME_ACTIVITY_QUIET_MS;
    initial.capture_completed_at_ms = initial.capture_started_at_ms;
    RuntimeActivityProof {
        schema_version: 1,
        initial,
        final_observation,
    }
}

async fn transaction(store: &MongoSandboxStore) -> ClientSession {
    let mut session = store.client.start_session().await.unwrap();
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    session
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn read_only_observer_uses_snapshot_watermark_and_content_free_queries() {
    let (admin, name, original) = database().await;
    let (sandbox, _) = fixture(&original, "sbx_activity_read").await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let trace = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    trace.lock().unwrap().clear();
    let before = original
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let snapshot = observe(&store, &sandbox.id).await;
    assert!(!snapshot.has_activity());
    assert_eq!(snapshot.vector.agents.len(), 1);
    assert_eq!(snapshot.vector.log_tails.len(), 1);
    assert!(snapshot.watermark.seconds > 0);
    let commands = trace.lock().unwrap().clone();
    let mut finds = 0;
    for (name, command) in &commands {
        assert!(
            matches!(name.as_str(), "hello" | "find" | "getMore" | "killCursors"),
            "observer command {name}"
        );
        if name == "find" {
            finds += 1;
            let concern = command.get_document("readConcern").unwrap();
            assert_eq!(concern.get_str("level").unwrap(), "snapshot");
            assert!(!command
                .get_document("projection")
                .unwrap()
                .contains_key("record"));
            if let Ok(at) = concern.get_timestamp("atClusterTime") {
                assert_eq!(
                    (at.time, at.increment),
                    (snapshot.watermark.seconds, snapshot.watermark.increment)
                );
            }
        }
    }
    assert!(finds >= 7);
    assert_eq!(
        before,
        original
            .sandboxes
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap()
    );
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_compares_original_vector_and_fences_without_refreshing_proof() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_admit").await;
    let proof = fixture_proof(observe(&store, &sandbox.id).await);
    let original = proof.clone();
    let root = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = transaction(&store).await;
    validate_for_admission(
        &store,
        &root,
        std::slice::from_ref(&agent),
        &proof,
        now_ms().unwrap(),
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    for (collection, id) in [
        (&store.sandboxes, sandbox.id.as_str()),
        (&store.agents, agent.id.as_str()),
    ] {
        let row = collection.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        assert_eq!(row.get_i64("runtime_migration_admission_fence").unwrap(), 1);
    }
    assert_eq!(proof, original);
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$inc":{"desktop_activity_fence":1_i64}},
        )
        .await
        .unwrap();
    let mut session = transaction(&store).await;
    assert!(validate_for_admission(
        &store,
        &root,
        &[agent],
        &proof,
        now_ms().unwrap(),
        &mut session
    )
    .await
    .is_err());
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_parent_or_agent_writer_conflicts_with_admission_fence() {
    let (client, name, store) = database().await;
    let other = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    for child in [false, true] {
        let (sandbox, agent) = fixture(
            &store,
            if child {
                "sbx_activity_agent_race"
            } else {
                "sbx_activity_root_race"
            },
        )
        .await;
        let proof = fixture_proof(observe(&store, &sandbox.id).await);
        let root = store
            .sandboxes
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut session = transaction(&store).await;
        capture::current(
            &store,
            &sandbox.id,
            proof.source_context(),
            &mut session,
            false,
        )
        .await
        .unwrap();
        let (collection, id) = if child {
            ("agents", agent.id.as_str())
        } else {
            ("sandboxes", sandbox.id.as_str())
        };
        other
            .database(&name)
            .collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$inc":{"agent_command_admission_fence":1_i64}},
            )
            .await
            .unwrap();
        let result = validate_for_admission(
            &store,
            &root,
            &[agent],
            &proof,
            now_ms().unwrap(),
            &mut session,
        )
        .await;
        assert!(
            matches!(result, Err(ApplyError::Driver(ref error)) if crate::store::sandbox_mongo_transactions::is_transient(error)),
            "{result:?}"
        );
        session.abort_transaction().await.unwrap();
        assert!(!store
            .sandboxes
            .find_one(doc! {"_id":sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .contains_key("runtime_migration_admission_fence"));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unsupported_context_corrupt_log_and_overflow_stay_unknown() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_unknown").await;
    let mut wrong = context(&store);
    wrong.replica_mode = true;
    assert!(matches!(
        store
            .observe_runtime_activity(&sandbox.id, &wrong)
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(_)
    ));
    let renewed = store.clone().with_renewable_work(true);
    assert!(matches!(
        renewed
            .observe_runtime_activity(&sandbox.id, &context(&store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(_)
    ));
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str()},
            doc! {"$set":{"record.seq":2_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .observe_runtime_activity(&sandbox.id, &context(&store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(_)
    ));
    let empty = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_activity_overflow",
    );
    store.create_sandbox(empty.clone()).await.unwrap();
    store
        .workspaces
        .insert_many(
            (0..=RUNTIME_ACTIVITY_MAX_ROWS).map(
                |i| doc! {"_id":format!("wks_overflow_{i:04}"), "sandbox_id":empty.id.as_str()},
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .observe_runtime_activity(&empty.id, &context(&store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(_)
    ));
    client.database(&name).drop().await.unwrap();
}

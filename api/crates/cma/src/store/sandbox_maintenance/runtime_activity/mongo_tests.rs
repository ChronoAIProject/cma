use std::sync::{Arc, Mutex};

use layer2_core::sandbox::store::SandboxStore;
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use mongodb::options::{ClientOptions, ReadConcern, WriteConcern};
use mongodb::Client;

use super::*;

mod generation;
mod lease;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    database as base_database, fixture,
};

async fn database() -> (Client, String, MongoSandboxStore) {
    let (client, name, store) = base_database().await;
    // Provision the actual optional log schema in the disposable fixture only.
    // Observation never creates missing collections to manufacture coverage.
    crate::store::agent_log_segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    (client, name, store)
}

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

async fn transaction(store: &MongoSandboxStore, proof: &RuntimeActivityProof) -> ClientSession {
    let mut session = store
        .client
        .start_session()
        .causal_consistency(true)
        .await
        .unwrap();
    let watermark = proof.final_observation.watermark;
    session.advance_operation_time(mongodb::bson::Timestamp {
        time: watermark.seconds,
        increment: watermark.increment,
    });
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
    let (sandbox, agent) = fixture(&original, "sbx_activity_read").await;
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
            matches!(
                name.as_str(),
                "hello" | "listCollections" | "find" | "getMore" | "killCursors"
            ),
            "observer command {name}"
        );
        if name == "listCollections" {
            assert_eq!(
                command.get_document("filter").unwrap(),
                &doc! {"name":{"$in":storage::COLLECTIONS.to_vec()}}
            );
            assert_eq!(
                command
                    .get_document("cursor")
                    .unwrap()
                    .get_i32("batchSize")
                    .ok()
                    .map(i64::from)
                    .or_else(|| command
                        .get_document("cursor")
                        .unwrap()
                        .get_i64("batchSize")
                        .ok()),
                Some(storage::COLLECTIONS.len() as i64 + 1)
            );
        }
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
    // Admission starts a distinct causal transaction after the saved boundary.
    // The observer assertions above are evaluated before any admission writes.
    let proof = fixture_proof(snapshot);
    trace.lock().unwrap().clear();
    let mut session = transaction(&store, &proof).await;
    validate_for_admission(
        &store,
        &before,
        &[agent],
        &proof,
        now_ms().unwrap(),
        &mut session,
    )
    .await
    .unwrap();
    session.abort_transaction().await.unwrap();
    let commands = trace.lock().unwrap().clone();
    let first = commands
        .iter()
        .find(|(_, command)| matches!(command.get_bool("startTransaction"), Ok(true)))
        .unwrap();
    let lower = first
        .1
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("afterClusterTime")
        .unwrap();
    let watermark = proof.final_observation.watermark;
    assert_eq!(
        (lower.time, lower.increment),
        (watermark.seconds, watermark.increment)
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
    let mut session = transaction(&store, &proof).await;
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
    let mut session = transaction(&store, &proof).await;
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
        let mut session = transaction(&store, &proof).await;
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
            matches!(result, Err(ApplyError::Driver(ref error)) if crate::store::sandbox_mongo::is_transient(error)),
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
    let row = |i| {
        let workspace = crate::store::sandbox_mongo_integration_tests::ready_workspace(
            &empty,
            &format!("wks_overflow_{i:04}"),
            &format!("overflow-{i:04}"),
        );
        let document = crate::store::workspaces::workspace_document(&workspace).unwrap();
        assert_eq!(
            crate::store::workspaces::read_workspace(&document).unwrap(),
            workspace
        );
        document
    };
    store
        .workspaces
        .insert_many((0..RUNTIME_ACTIVITY_MAX_ROWS).map(row))
        .await
        .unwrap();
    // Valid quiet rows must produce a complete snapshot at the bound; otherwise
    // malformed fixture metadata could masquerade as the overflow refusal.
    let at_limit = observe(&store, &empty.id).await;
    assert_eq!(at_limit.vector.workspaces.len(), RUNTIME_ACTIVITY_MAX_ROWS);
    assert!(!at_limit.has_activity());
    store
        .workspaces
        .insert_one(row(RUNTIME_ACTIVITY_MAX_ROWS))
        .await
        .unwrap();
    assert!(matches!(
        store
            .observe_runtime_activity(&empty.id, &context(&store))
            .await
            .unwrap(),
        RuntimeActivityObservation::Unknown(RuntimeActivityUnknownReason::IncompleteCoverage)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn control_reservation_is_visible_even_if_the_control_settles_between_samples() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_activity_control").await;
    let before = observe(&store, &sandbox.id).await;
    assert!(!before.has_activity());
    let root = store
        .sandboxes
        .find_one(doc! {"_id": sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();

    // Model the durable projection of a reservation and its parent fence.
    let mut reserved = client.start_session().await.unwrap();
    reserved.start_transaction().await.unwrap();
    store
        .sandbox_controls
        .insert_one(
            doc! {"_id": "fixture-control", "sandbox_id": sandbox.id.as_str(), "queued": true},
        )
        .session(&mut reserved)
        .await
        .unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$inc": {"resource_share_guard": 1_i64}},
        )
        .session(&mut reserved)
        .await
        .unwrap();
    reserved.commit_transaction().await.unwrap();
    assert!(observe(&store, &sandbox.id).await.has_activity());
    let proof = fixture_proof(before.clone());
    let mut admission = transaction(&store, &proof).await;
    assert!(validate_for_admission(
        &store,
        &root,
        std::slice::from_ref(&agent),
        &proof,
        now_ms().unwrap(),
        &mut admission,
    )
    .await
    .is_err());
    admission.abort_transaction().await.unwrap();

    store
        .sandbox_controls
        .update_one(
            doc! {"_id": "fixture-control"},
            doc! {"$set": {"queued": false}},
        )
        .await
        .unwrap();
    let after = observe(&store, &sandbox.id).await;
    assert!(!after.has_activity());
    assert_eq!(
        after.vector.sandbox.revision,
        before.vector.sandbox.revision
    );
    assert_ne!(after.vector, before.vector);
    let mut proof = fixture_proof(after);
    proof.initial.vector = before.vector;
    assert!(proof.validate_at(now_ms().unwrap()).is_err());
    client.database(&name).drop().await.unwrap();
}

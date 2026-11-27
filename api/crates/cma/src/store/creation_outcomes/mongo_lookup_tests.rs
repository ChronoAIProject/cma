use std::sync::{Arc, Mutex};

use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};

use super::*;

#[path = "mongo_pre_validation_tests.rs"]
mod mongo_pre_validation_tests;

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn invalid_covered_lifecycle_submission_settles_before_the_sweeper() {
    use axum::{body::Body, http::Request, Extension};
    use contract::AuthorizedLookup;
    use mongo_lifecycle_route_tests::{call, routes};

    let (mut store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    const BODY: &str = r#"{"name":""}"#;
    for terminal_settlement in [false, true] {
        store.config.terminal_settlement = terminal_settlement;
        let (app, _, _workers_held) = routes(&store).await;
        let lookup = contract::router(Arc::new(reader.clone()))
            .layer(Extension(AuthorizedLookup(owner.clone())));
        let nonce = if terminal_settlement { "a" } else { "b" }.repeat(32);
        let digest = contract::wire_digest(BODY.as_bytes());
        let epoch = &store.config.epoch;
        let path = Operation::AgentBootstrap.path();
        let binding = contract::hash_fields(&[
            "cma-creation-key-v1",
            epoch,
            &today.to_string(),
            &nonce,
            owner.as_str(),
            store.deployment(),
            "POST",
            path,
            &digest,
        ]);
        let key = format!("cma1.{epoch}.{today}.{nonce}.{binding}");
        let submit = || {
            Request::post(path)
                .header("content-type", "application/json")
                .header("Idempotency-Key", &key)
                .body(Body::from(BODY))
                .unwrap()
        };
        let lookup_request = || {
            Request::post(contract::LOOKUP_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "version": 1,
                        "deployment": store.deployment(),
                        "method": "POST",
                        "path": path,
                        "idempotency_key": key,
                        "fingerprint": {"algorithm": "nyxid-wire-v1", "digest": digest},
                    })
                    .to_string(),
                ))
                .unwrap()
        };
        call(&app, submit(), StatusCode::UNPROCESSABLE_ENTITY).await;
        let first = call(&lookup, lookup_request(), StatusCode::OK).await;
        assert_eq!(first["result"]["operation_id"], serde_json::Value::Null);
        if terminal_settlement {
            assert_eq!(first["result"]["kind"], "refused");
            assert_eq!(first["result"]["code"], "creation_failed");
            assert!(contract::lower_hex(
                first["result"]["proof_id"].as_str().unwrap(),
                64
            ));
        } else {
            assert_eq!(first["result"]["kind"], "in_progress");
        }
        call(&app, submit(), StatusCode::CONFLICT).await;
        assert_eq!(call(&lookup, lookup_request(), StatusCode::OK).await, first);
    }
    for collection in [
        "sandboxes",
        super::super::super::agent_lifecycle_operations::COLLECTION,
    ] {
        assert_eq!(
            store
                .database
                .collection::<Document>(collection)
                .count_documents(doc! {"owner":owner.as_str()})
                .await
                .unwrap(),
            0,
        );
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn rotated_reader_retains_terminal_and_closed_window_proofs() {
    let (store, mut rotated, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let request = query(
        &store,
        &owner,
        Operation::AgentBootstrap,
        today,
        &"a".repeat(32),
    );
    let original = identity(&store, &owner, &request);
    let Begin::Acquired(fence) = store.begin(&original).await.unwrap() else {
        panic!("expected original submission fence");
    };
    rotated.config.epoch = "f".repeat(32);
    let unknown = Outcome::Unknown {
        reason: UnknownReason::CoverageUnavailable,
    };
    assert_eq!(rotated.lookup(&original, &request).await.unwrap(), unknown);
    store
        .records
        .update_one(
            doc! {"_id":&fence.record_id},
            doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    store.settle_expired_unlinked().await.unwrap();
    let terminal = store.lookup(&original, &request).await.unwrap();
    assert!(matches!(
        terminal,
        Outcome::Refused {
            code: RefusalCode::CreationExpired,
            ..
        }
    ));
    assert_eq!(rotated.lookup(&original, &request).await.unwrap(), terminal);
    assert!(matches!(
        rotated.begin(&original).await.unwrap(),
        Begin::Closed
    ));

    let mut missing = query(
        &store,
        &owner,
        Operation::AgentBootstrap,
        today - 2,
        &"b".repeat(32),
    );
    let identity_for_miss = identity(&store, &owner, &missing);
    assert_eq!(
        rotated.lookup(&identity_for_miss, &missing).await.unwrap(),
        unknown
    );
    store.close_elapsed_days().await.unwrap();
    let closed_miss = store.lookup(&identity_for_miss, &missing).await.unwrap();
    assert!(matches!(closed_miss, Outcome::NeverReceived { .. }));
    assert_eq!(
        rotated.lookup(&identity_for_miss, &missing).await.unwrap(),
        closed_miss
    );
    assert!(matches!(
        store.begin(&identity_for_miss).await.unwrap(),
        Begin::Closed
    ));

    let before_coverage = query(
        &store,
        &owner,
        Operation::AgentBootstrap,
        today - 4,
        &"b".repeat(32),
    );
    assert_eq!(
        rotated
            .lookup(
                &identity(&store, &owner, &before_coverage),
                &before_coverage
            )
            .await
            .unwrap(),
        unknown
    );
    missing.idempotency_key = "uncovered-opaque-fixture".into();
    assert_eq!(
        rotated
            .lookup(&identity(&rotated, &owner, &missing), &missing)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::NoDurableEvidence
        }
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn closed_miss_uses_one_read_only_snapshot_with_point_reads() {
    let (store, _, today) = fixture().await;
    let owner = SubjectId::new("monitored-owner");
    let request = query(
        &store,
        &owner,
        Operation::Sandbox,
        today - 2,
        &"a".repeat(32),
    );
    let original = identity(&store, &owner, &request);
    store.close_elapsed_days().await.unwrap();

    let trace = Arc::new(Mutex::new(Vec::new()));
    let captured = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let app_name = format!(
        "creation-snapshot-fixture-{}",
        uuid::Uuid::new_v4().simple()
    );
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            captured
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let database = mongodb::Client::with_options(options)
        .unwrap()
        .database(store.database.name());
    let reader = CreationOutcomes {
        records: majority_collection(&database, COLLECTION),
        database,
        config: store.config.clone(),
    };
    assert!(matches!(
        reader.lookup(&original, &request).await.unwrap(),
        Outcome::NeverReceived { .. }
    ));
    let commands = trace.lock().unwrap().clone();
    assert_eq!(
        commands
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["find", "find", "find"]
    );
    let first_read = &commands[0].1;
    assert_eq!(
        first_read
            .get_document("readConcern")
            .unwrap()
            .get_str("level")
            .unwrap(),
        "snapshot"
    );
    assert_eq!(
        first_read
            .get_document("filter")
            .unwrap()
            .get_str("owner")
            .unwrap(),
        owner.as_str()
    );
    assert!(commands[1]
        .1
        .get_document("filter")
        .unwrap()
        .contains_key("_id"));
    assert!(commands[2]
        .1
        .get_document("filter")
        .unwrap()
        .contains_key("subject_hash"));
    let session = first_read.get_document("lsid").unwrap();
    let mut snapshot_times = Vec::new();
    for (index, (_, command)) in commands.iter().enumerate() {
        assert_eq!(command.get_document("lsid").unwrap(), session);
        assert!(!command.contains_key("txnNumber"));
        let concern = command.get_document("readConcern").unwrap();
        assert_eq!(concern.get_str("level").unwrap(), "snapshot");
        if index > 0 {
            snapshot_times.push(concern.get_timestamp("atClusterTime").unwrap());
        }
        assert!(matches!(
            command.get("maxTimeMS"),
            Some(mongodb::bson::Bson::Int32(750) | mongodb::bson::Bson::Int64(750))
        ));
    }
    assert_eq!(snapshot_times[0], snapshot_times[1]);
    let admin = store.database.client().database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times":1},
            "data": {"failCommands":["find"], "appName":app_name, "errorCode":246},
        })
        .await
        .unwrap();
    let result = reader.lookup(&original, &request).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(StatusCode::SERVICE_UNAVAILABLE));
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn opaque_lifecycle_key_requires_original_wire_evidence_and_exact_new_origin() {
    exercise_opaque_lifecycle_lookup(false).await;
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn opaque_lifecycle_lookup_reads_a_managed_desktop_envelope() {
    exercise_opaque_lifecycle_lookup(true).await;
}

async fn exercise_opaque_lifecycle_lookup(desktop: bool) {
    let (store, reader, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let operations = crate::store::MongoAgentLifecycleStore::open(&store.database)
        .await
        .unwrap();
    let mut sandbox_store =
        crate::store::MongoSandboxStore::open(store.database.client(), store.database.name())
            .await
            .unwrap();
    let mut request = query(
        &store,
        &owner,
        Operation::AgentBootstrap,
        today,
        &"b".repeat(32),
    );
    request.idempotency_key = "opaque-wire-fixture".into();
    let mut input = crate::store::LifecycleInput {
        behavior: Default::default(),
        managed_agent_instructions: None,
        setup_message: None,
        request: serde_json::from_value(serde_json::json!({"name":"Fixture lifecycle"})).unwrap(),
        resolved_image: None,
        capacity: None,
        sandbox_availability: Default::default(),
        capabilities: vec![layer2_core::Capability::UseOwnSandboxes],
        actor: None,
    };
    if desktop {
        sandbox_store.runtime_image_origin_enabled = true;
        input.behavior = crate::store::LifecycleBehavior::PromptRepository;
        input.request.desktop = true;
        input.resolved_image = Some(format!(
            "registry.example/runtime@sha256:{}",
            "a".repeat(64)
        ));
    }
    let crate::store::CreateResult::Created(id) = operations
        .create_with_submission(
            &owner,
            &request.idempotency_key,
            input.clone(),
            Some(contract::Submission {
                deployment: request.deployment.clone(),
                wire_digest: request.fingerprint.digest.clone(),
                fence: None,
                replay_operation_id: None,
            }),
            None,
        )
        .await
        .unwrap()
    else {
        panic!("expected first lifecycle admission")
    };
    request.operation_id = Some(id.clone());
    let original = identity(&store, &owner, &request);
    assert_eq!(
        reader.lookup(&original, &request).await.unwrap(),
        Outcome::InProgress {
            operation_id: Some(id.clone())
        }
    );

    let mut candidate = super::super::super::sandbox_admission_mongo_integration_tests::sandbox(
        "sbx_wire_fixture",
        "unused",
    );
    candidate.origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
        operation_id: id.clone(),
    };
    if desktop {
        let key = format!("agent-manager:{}:sandbox", request.idempotency_key);
        candidate.profile.id =
            layer2_core::profiles::private_sandbox_profile_id(&owner, &key).unwrap();
        candidate
            .profile
            .content
            .runtime_image
            .clone_from(&input.resolved_image);
        candidate.profile.digest =
            layer2_core::profiles::content_digest(&candidate.profile.content).unwrap();
    }
    let claimed = operations.claim(&id).await.unwrap().unwrap();
    let lease = claimed.lease().unwrap();
    let admission = SandboxAdmission::new(
        candidate,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        layer2_core::now_rfc3339(),
    )
    .unwrap()
    .with_lifecycle_lease(&lease.token);
    sandbox_store.admit_sandbox(admission).await.unwrap();
    let row = sandbox_store
        .sandboxes
        .find_one(doc! {"_id":"sbx_wire_fixture"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_i64("document_schema_version").unwrap(),
        if desktop { 6 } else { 1 }
    );
    assert!(matches!(reader.lookup(&original, &request).await.unwrap(),
        Outcome::Created { sandbox_id, operation_id: Some(operation), .. } if sandbox_id == "sbx_wire_fixture" && operation == id));

    request.fingerprint.digest = contract::wire_digest(b"{}");
    assert_eq!(
        reader
            .lookup(&identity(&store, &owner, &request), &request)
            .await,
        Err(StatusCode::CONFLICT)
    );
    let crate::store::CreateResult::Created(old_id) = operations
        .create(&owner, "old-opaque-fixture", input)
        .await
        .unwrap()
    else {
        panic!("expected older lifecycle admission")
    };
    request.idempotency_key = "old-opaque-fixture".into();
    request.operation_id = Some(old_id);
    assert_eq!(
        reader
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::LegacyFingerprintUnavailable
        }
    );
    store.database.drop().await.unwrap();
}

use super::*;
use contract::{Algorithm, RequestFingerprint, UnknownReason};
use layer2_core::sandbox::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore};

#[path = "mongo_lifecycle_route_tests.rs"]
mod mongo_lifecycle_route_tests;
#[path = "mongo_lookup_tests.rs"]
mod mongo_lookup_tests;

const DAY_MS: i64 = 86_400_000;

async fn fixture() -> (CreationOutcomes, CreationOutcomes, i64) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable local replica set");
    let first = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let second = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_creation_fixture_{}", uuid::Uuid::new_v4().simple());
    let database = first.database(&name);
    let today = DateTime::now().timestamp_millis().div_euclid(DAY_MS);
    let config = CreationOutcomesConfig {
        deployment: "d".repeat(64),
        epoch: "e".repeat(32),
        first_day: today - 3,
        legacy_custody: true,
        terminal_settlement: true,
        validation_settlement: false,
    };
    let id = format!("coverage:{}:{}", config.deployment, config.epoch);
    database.collection::<Document>(COLLECTION).insert_one(doc! {
        "_id":id,"document_schema_version":VERSION,"first_day":config.first_day,"closed_day":config.first_day - 1,"admissions":0_i64,
    }).await.unwrap();
    let left = CreationOutcomes::open(database, config.clone())
        .await
        .unwrap();
    let right = CreationOutcomes::open(second.database(&name), config)
        .await
        .unwrap();
    (left, right, today)
}

fn identity<'a>(
    store: &'a CreationOutcomes,
    owner: &'a SubjectId,
    request: &'a LookupRequest,
) -> Identity<'a> {
    request.identity(owner, store.deployment()).unwrap()
}

fn query(
    store: &CreationOutcomes,
    owner: &SubjectId,
    operation: Operation,
    day: i64,
    nonce: &str,
) -> LookupRequest {
    let day = day.to_string();
    let digest = contract::wire_digest(b"{ }");
    let deployment = store.deployment();
    let epoch = &store.config.epoch;
    let binding = contract::hash_fields(&[
        "cma-creation-key-v1",
        epoch,
        &day,
        nonce,
        owner.as_str(),
        deployment,
        "POST",
        operation.path(),
        &digest,
    ]);
    LookupRequest {
        version: 1,
        deployment: deployment.into(),
        method: "POST".into(),
        path: operation.path().into(),
        idempotency_key: format!("cma1.{epoch}.{day}.{nonce}.{binding}"),
        fingerprint: RequestFingerprint {
            algorithm: Algorithm::NyxidWire,
            digest,
        },
        claim_id: None,
        operation_id: None,
    }
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn closed_miss_is_immutable_and_cannot_be_admitted_by_another_client() {
    let (store, other, today) = fixture().await;
    let owner = SubjectId::new("fixture-owner");
    let request = query(
        &store,
        &owner,
        Operation::Sandbox,
        today - 2,
        &"1".repeat(32),
    );
    let original = identity(&store, &owner, &request);
    assert!(matches!(
        store.lookup(&original, &request).await.unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::CoverageUnavailable
        }
    ));
    store.close_elapsed_days().await.unwrap();
    let terminal = other.lookup(&original, &request).await.unwrap();
    assert!(matches!(terminal, Outcome::NeverReceived { .. }));
    assert!(matches!(
        other.begin(&original).await.unwrap(),
        Begin::Closed
    ));
    store.close_elapsed_days().await.unwrap();
    assert_eq!(store.lookup(&original, &request).await.unwrap(), terminal);
    assert_eq!(
        store
            .records
            .count_documents(doc! {"owner":owner.as_str()})
            .await
            .unwrap(),
        0
    );
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn exact_sandbox_transaction_and_refusal_contend_on_one_durable_generation() {
    let (store, other, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let mongo =
        crate::store::MongoSandboxStore::open(store.database.client(), store.database.name())
            .await
            .unwrap();
    let mut mongo = mongo;
    mongo.creation_writer_epoch = Some(store.config.epoch.clone());
    let request = query(&store, &owner, Operation::Sandbox, today, &"2".repeat(32));
    let original = identity(&store, &owner, &request);
    let Begin::Acquired(fence) = store.begin(&original).await.unwrap() else {
        panic!("first writer must acquire")
    };
    assert!(matches!(
        other.begin(&original).await.unwrap(),
        Begin::Existing { .. }
    ));
    assert!(matches!(
        other.lookup(&original, &request).await.unwrap(),
        Outcome::InProgress { .. }
    ));
    let mut sandbox =
        super::super::sandbox_admission_mongo_integration_tests::sandbox("sbx_evidence", "unused");
    sandbox.origin = layer2_core::sandbox::SandboxOrigin::Manual;
    let mut admission = SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    admission.creation_fence = Some(fence.clone());
    let (creation, refused) = tokio::join!(
        mongo.admit_sandbox(admission.clone()),
        other.refuse_direct(&owner, &fence, RefusalCode::SandboxLimitReached)
    );
    let proof = other.lookup(&original, &request).await.unwrap();
    match (creation, refused.unwrap(), proof) {
        (Ok(_), false, Outcome::Created { sandbox_id, .. }) => {
            assert_eq!(sandbox_id, "sbx_evidence")
        }
        (
            Err(_),
            true,
            Outcome::Refused {
                code: RefusalCode::SandboxLimitReached,
                ..
            },
        ) => {}
        _ => panic!("the committed effect and refusal must exclude each other"),
    }
    assert!(mongo.admit_sandbox(admission).await.is_err());
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn legacy_receipt_requires_original_generation_owner_and_live_bounded_evidence() {
    use mongodb::bson::{spec::BinarySubtype, Binary};
    use sha2::{Digest, Sha256};

    let (store, other, _) = fixture().await;
    let owner = SubjectId::new("receipt-owner");
    let foreign = SubjectId::new("foreign-owner");
    let key = "legacy-fixture";
    let claim = "4".repeat(32);
    let fingerprint = Sha256::digest(b"{ }");
    let mut request = LookupRequest {
        version: 1,
        deployment: store.deployment().to_owned(),
        method: "POST".into(),
        path: Operation::Sandbox.path().into(),
        idempotency_key: key.into(),
        fingerprint: RequestFingerprint {
            algorithm: Algorithm::Sha256Wire,
            digest: format!("{fingerprint:x}"),
        },
        claim_id: Some(claim.clone()),
        operation_id: None,
    };
    let id = doc! {"subject":owner.as_str(), "operation":"sandbox:create", "key":key};
    let claims = store
        .database
        .collection::<Document>(super::super::operation::OPERATIONS);
    let response = serde_json::to_vec(&serde_json::json!({
        "status":201, "headers":[],
        "body":serde_json::to_vec(&serde_json::json!({"id":"sbx_legacy","owner":owner.as_str()})).unwrap(),
    })).unwrap();
    claims
        .insert_one(doc! {
            "_id": id.clone(), "document_schema_version":VERSION, "claim_id": &claim,
            "fingerprint": Binary {subtype:BinarySubtype::Generic, bytes:fingerprint.to_vec()},
            "outcome": Binary {subtype:BinarySubtype::Generic, bytes:response},
            "expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()+120_000),
        })
        .await
        .unwrap();
    assert!(matches!(
        other
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Created { .. }
    ));
    assert!(matches!(
        other
            .lookup(&identity(&store, &foreign, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::NoDurableEvidence
        }
    ));
    request.claim_id = Some("5".repeat(32));
    assert_eq!(
        other
            .lookup(&identity(&store, &owner, &request), &request)
            .await,
        Err(StatusCode::CONFLICT)
    );
    request.claim_id = None;
    assert!(matches!(
        other
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::GenerationAmbiguous
        }
    ));
    request.claim_id = Some(claim);
    claims
        .update_one(
            doc! {"_id":id.clone()},
            doc! {"$set":{"outcome":Binary {subtype:BinarySubtype::Generic,bytes:vec![0;65_537]}}},
        )
        .await
        .unwrap();
    assert!(matches!(
        other
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::RecoveryRequired
        }
    ));
    claims
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"expires_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(matches!(
        other
            .lookup(&identity(&store, &owner, &request), &request)
            .await
            .unwrap(),
        Outcome::Unknown {
            reason: UnknownReason::NoDurableEvidence
        }
    ));
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn closure_waits_for_an_admitted_writer_and_a_closed_key_cannot_start() {
    let (left, right, today) = fixture().await;
    let owner = SubjectId::new("race-owner");
    let request = query(&left, &owner, Operation::Sandbox, today, &"7".repeat(32));
    let original = identity(&left, &owner, &request);
    let mut writer = left.database.client().start_session().await.unwrap();
    transaction::start_transaction(&mut writer).await.unwrap();
    let result = left
        .begin_transaction(&original, &mut writer)
        .await
        .ok()
        .unwrap();
    assert!(matches!(result, Begin::Acquired(_)));

    // Force this fixture's day boundary while the first-admission transaction
    // still owns the same document. The production tick uses this same write.
    let mut closer = tokio::spawn({
        let right = right.clone();
        async move {
            right.records.update_one(doc! {"_id": format!("coverage:{}:{}",right.config.deployment,right.config.epoch)},
                doc! {"$max":{"closed_day":today}}).await.unwrap();
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut closer)
            .await
            .is_err()
    );
    assert!(matches!(
        transaction::commit_transaction(&mut writer).await,
        transaction::CommitOutcome::Committed
    ));
    closer.await.unwrap();
    assert!(matches!(
        right.lookup(&original, &request).await.unwrap(),
        Outcome::InProgress { .. }
    ));

    let missed = query(&left, &owner, Operation::Sandbox, today, &"8".repeat(32));
    let missed = identity(&left, &owner, &missed);
    assert!(matches!(right.begin(&missed).await.unwrap(), Begin::Closed));
    left.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn lifecycle_links_original_wire_and_fences_a_stale_worker() {
    use crate::store::released_runtimes::MongoReleasedRuntimes;
    use crate::store::{
        CreateResult, LifecycleCreateError, LifecycleInput, MongoAgentLifecycleStore,
    };
    use contract::Submission;
    use layer2_core::released_runtime::{ReleasedLane, ReleasedLaneRecord};
    use layer3_adapter::{ReleasedLaneRevisions, ReleasedRuntime};

    let (left, right, today) = fixture().await;
    let owner = SubjectId::new("owner");
    let mut operations = MongoAgentLifecycleStore::open(&left.database)
        .await
        .unwrap();
    operations.creation_writer_epoch = Some(left.config.epoch.clone());
    let lanes = MongoReleasedRuntimes::open(&left.database, "creation-release-test");
    let lane = ReleasedLane::Agent;
    let image = |digit: char| {
        format!(
            "registry.example/runtime@sha256:{}",
            digit.to_string().repeat(64)
        )
    };
    let active = lanes
        .bootstrap(
            ReleasedLaneRecord::bootstrap(lane, image('b'), &layer2_core::now_rfc3339()).unwrap(),
        )
        .await
        .unwrap();
    let release = ReleasedRuntime {
        revision: active.revision,
        agent: active.active.image,
        machine: None,
        lane_revisions: Some(ReleasedLaneRevisions {
            agent: active.active.revision,
            machine: None,
        }),
    };
    operations.released_lanes = Some(lanes.clone());
    let mut sandbox_store =
        crate::store::MongoSandboxStore::open(left.database.client(), left.database.name())
            .await
            .unwrap();
    sandbox_store.creation_writer_epoch = Some(left.config.epoch.clone());
    let query = query(
        &left,
        &owner,
        Operation::AgentBootstrap,
        today,
        &"9".repeat(32),
    );
    let original = identity(&left, &owner, &query);
    let Begin::Acquired(mut fence) = left.begin(&original).await.unwrap() else {
        panic!("first request must own the proof generation")
    };
    let input = LifecycleInput {
        behavior: Default::default(),
        managed_agent_instructions: None,
        setup_message: None,
        request: serde_json::from_value(
            serde_json::json!({"name":"Fixture lifecycle", "desktop": true}),
        )
        .unwrap(),
        resolved_image: Some(release.agent.clone()),
        capacity: None,
        sandbox_availability: Default::default(),
        capabilities: vec![layer2_core::Capability::UseOwnSandboxes],
        actor: None,
    };
    let mut submission = Submission {
        deployment: original.deployment.to_owned(),
        wire_digest: original.wire_digest.to_owned(),
        fence: Some(fence.clone()),
        replay_operation_id: None,
    };
    let stale_release = ReleasedRuntime {
        agent: image('a'),
        ..release.clone()
    };
    let stale_input = LifecycleInput {
        resolved_image: Some(stale_release.agent.clone()),
        ..input.clone()
    };
    assert!(matches!(
        operations
            .create_with_submission(
                &owner,
                original.key,
                stale_input,
                Some(submission.clone()),
                Some((&stale_release, lane))
            )
            .await,
        Err(LifecycleCreateError::StaleReleasedRuntime)
    ));
    submission.fence.as_mut().unwrap().generation = "0".repeat(32);
    assert!(operations
        .create_with_submission(
            &owner,
            original.key,
            input.clone(),
            Some(submission.clone()),
            Some((&release, lane))
        )
        .await
        .is_err());
    assert_eq!(
        right.lookup(&original, &query).await.unwrap(),
        Outcome::InProgress { operation_id: None }
    );
    assert_eq!(lanes.load(lane).await.unwrap().unwrap().admissions, 0);
    assert!(operations
        .replay(&owner, original.key, &input.request)
        .await
        .unwrap()
        .is_none());

    submission.fence = Some(fence.clone());
    let CreateResult::Created(id) = operations
        .create_with_submission(
            &owner,
            original.key,
            input,
            Some(submission),
            Some((&release, lane)),
        )
        .await
        .unwrap()
    else {
        panic!("expected one linked operation")
    };
    assert_eq!(
        right.lookup(&original, &query).await.unwrap(),
        Outcome::InProgress {
            operation_id: Some(id.clone())
        }
    );
    assert_eq!(lanes.load(lane).await.unwrap().unwrap().admissions, 1);
    let lease = operations.claim(&id).await.unwrap().unwrap();
    assert_eq!(lease.released_lane, Some(lane));
    let lease = lease.lease().unwrap();
    fence.lifecycle_lease = Some((id.clone(), "stale-token-fixture".into()));
    let mut candidate = super::super::sandbox_admission_mongo_integration_tests::sandbox(
        "sbx_lifecycle_proof",
        "unused",
    );
    candidate.origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
        operation_id: id.clone(),
    };
    let mut admission = SandboxAdmission::new(
        candidate,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    admission.creation_fence = Some(fence.clone());
    assert!(sandbox_store
        .admit_sandbox(admission.clone())
        .await
        .is_err());
    fence.lifecycle_lease = Some((id.clone(), lease.token.clone()));
    admission.creation_fence = Some(fence.clone());
    sandbox_store.admit_sandbox(admission).await.unwrap();
    assert!(!operations
        .fail_fenced_creation(
            &owner,
            &lease,
            &fence,
            "synthetic refusal",
            None,
            RefusalCode::SandboxLimitReached
        )
        .await
        .unwrap());
    assert!(matches!(right.lookup(&original, &query).await.unwrap(),
        Outcome::Created { sandbox_id, operation_id:Some(operation), .. } if sandbox_id == "sbx_lifecycle_proof" && operation == id));
    left.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn expired_covered_submission_publishes_stable_terminal_evidence() {
    let (store, other, today) = fixture().await;
    let owner = SubjectId::new("fixture-owner");
    let request = query(
        &store,
        &owner,
        Operation::AgentBootstrap,
        today,
        &"9".repeat(32),
    );
    let identity = identity(&store, &owner, &request);
    let Begin::Acquired(fence) = store.begin(&identity).await.unwrap() else {
        panic!("fixture admission missing")
    };
    store
        .records
        .update_one(
            doc! {"_id":&fence.record_id},
            doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    for i in 0..32 {
        store
            .records
            .insert_one(doc! {
                "_id":format!("malformed-{i}"), "document_schema_version":VERSION,
                "deployment":&store.config.deployment,"epoch":&store.config.epoch,
                "operation":"agent_bootstrap","owner":"fixture-owner","identity":"invalid",
                "generation":"invalid","state":{"kind":"pending"},
                "lease_until":DateTime::from_millis(-1000)
            })
            .await
            .unwrap();
    }
    for _ in 0..32 {
        store
            .records
            .insert_one(doc! {
                "_id":mongodb::bson::oid::ObjectId::new(), "document_schema_version":VERSION,
                "deployment":&store.config.deployment,"epoch":&store.config.epoch,
                "operation":"agent_bootstrap","owner":"fixture-owner","identity":"a".repeat(64),
                "generation":"b".repeat(32),"state":{"kind":"pending"},
                "lease_until":DateTime::from_millis(-2000)
            })
            .await
            .unwrap();
    }
    let (left, right) = tokio::join!(
        store.settle_expired_unlinked(),
        other.settle_expired_unlinked()
    );
    left.unwrap();
    right.unwrap();
    let proof = other.lookup(&identity, &request).await.unwrap();
    assert!(matches!(
        proof,
        Outcome::Refused {
            code: RefusalCode::CreationExpired,
            operation_id: None,
            ..
        }
    ));
    assert_eq!(proof, store.lookup(&identity, &request).await.unwrap());
    assert!(matches!(
        store.begin(&identity).await.unwrap(),
        Begin::Existing { .. }
    ));
    store.database.drop().await.unwrap();
}

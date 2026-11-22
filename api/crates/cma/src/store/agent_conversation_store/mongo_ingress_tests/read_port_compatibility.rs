use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use layer2_core::sandbox::{ingress_ownership::AdmittedError, store::SandboxStoreError, Agent};
use mongodb::{
    bson::{self, doc, Document},
    ClientSession, Database,
};

use super::{fixture::Fixture, policy, AllocationPlan, IngressClass};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        incarnation,
        publication::{self, PublicationIdentity},
        writer_admission::{self, Admission, AdmissionKind},
        writer_gate::{GateName, GateProof},
    },
    ingress_reads::error::ReadFailure,
};

fn fingerprint(byte: u8) -> bson::Binary {
    bson::Binary {
        subtype: bson::spec::BinarySubtype::Generic,
        bytes: vec![byte; 32],
    }
}

fn operation() -> PublicationIdentity {
    PublicationIdentity {
        key: "synthetic-admission".into(),
        fingerprint: fingerprint(7),
        appended_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn admission(agent: &Agent, operation: &PublicationIdentity) -> Admission {
    let incarnation = incarnation(agent);
    Admission {
        id: publication::identity(agent.id.as_str(), &incarnation, &operation.key),
        format_version: 1,
        agent_id: agent.id.0.clone(),
        incarnation,
        owner: agent.owner.as_str().into(),
        publication_key: operation.key.clone(),
        fingerprint: operation.fingerprint.clone(),
        appended_at: operation.appended_at.clone(),
        expected_agent_revision: agent.revision as i64,
        expected_seq: agent.conversation.last_seq as i64,
        admitted_at: operation.appended_at.clone(),
        kind: AdmissionKind::Activation,
        writer_gate: GateProof {
            name: GateName::Segments,
            revision: 1,
        },
        recovery_gate: None,
    }
}

fn plan() -> AllocationPlan {
    AllocationPlan::checked("synthetic admission read", &[50 * 1024 * 1024]).unwrap()
}

async fn begin(session: &mut ClientSession) {
    crate::store::sandbox_mongo::start_transaction(session)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_read_preserves_same_transaction_values_and_independent_retention() {
    let fixture = Fixture::new("admission_retention").await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_admission::COLLECTION);
    let operation = operation();
    let original = admission(&fixture.agent, &operation);
    rows.insert_one(bson::serialize_to_document(&original).unwrap())
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    begin(&mut session).await;
    let legacy = writer_admission::read(&database, &fixture.agent, &operation, &mut session)
        .await
        .unwrap()
        .unwrap();
    let result = writer_admission::read_admitted(
        &database,
        &fixture.agent,
        &operation,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    let retained = result.as_ref().unwrap().clone();
    assert_eq!(
        bson::serialize_to_document(&*retained).unwrap(),
        bson::serialize_to_document(&legacy).unwrap()
    );
    drop(result);
    assert!(retained.belongs_to(&policy));
    assert!(root.available_bytes() < idle);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    let small_available = policy.budget(IngressClass::Data).available_bytes();
    let expanded = "s".repeat(512);
    let mut changed_agent = fixture.agent.clone();
    changed_agent.owner = layer2_core::SubjectId(expanded.clone());
    rows.update_one(doc! {"_id":&original.id}, doc! {"$set":{"owner":&expanded}})
        .session(&mut session)
        .await
        .unwrap();
    let changed = writer_admission::read_admitted(
        &database,
        &changed_agent,
        &operation,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(changed.as_ref().unwrap().owner, expanded);
    assert_eq!(retained.owner, original.owner);
    assert!(
        small_available - policy.budget(IngressClass::Data).available_bytes() >= expanded.len()
    );
    session.abort_transaction().await.unwrap();
    drop(changed);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        small_available
    );
    begin(&mut session).await;
    let later = writer_admission::read_admitted(
        &database,
        &fixture.agent,
        &operation,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(later.as_ref().unwrap().owner, original.owner);
    drop((later, retained));
    assert_eq!(root.available_bytes(), idle);
    session.abort_transaction().await.unwrap();
    drop((session, rows, database));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_read_keeps_absence_conflict_corruption_and_preflight_refusal_distinct() {
    let fixture = Fixture::new("admission_errors").await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_admission::COLLECTION);
    let operation = operation();
    let valid = bson::serialize_to_document(&admission(&fixture.agent, &operation)).unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let absent = writer_admission::read_admitted(
        &database,
        &fixture.agent,
        &operation,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert!(absent.is_none());
    drop(absent);
    for (field, replacement, conflict) in [
        ("owner", bson::Bson::String("another-owner".into()), false),
        ("fingerprint", bson::Bson::Binary(fingerprint(8)), true),
        (
            "appended_at",
            bson::Bson::String("2026-01-02T00:00:00Z".into()),
            true,
        ),
        ("format_version", bson::Bson::Int64(99), false),
        ("expected_seq", bson::Bson::Int32(0), false),
        (
            "unexpected",
            bson::Bson::String("x".repeat(64 * 1024)),
            false,
        ),
    ] {
        let mut row = valid.clone();
        row.insert(field, replacement);
        rows.replace_one(doc! {"_id":valid.get_str("_id").unwrap()}, row)
            .upsert(true)
            .bypass_document_validation(true)
            .await
            .unwrap();
        let legacy = writer_admission::read(&database, &fixture.agent, &operation, &mut session)
            .await
            .unwrap_err();
        let TransactionLogError::Domain(legacy) = legacy else {
            panic!("domain became driver")
        };
        let error = writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            plan(),
        )
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap_err();
        let ReadFailure::Domain(domain) = &*error else {
            panic!("domain became driver/refusal")
        };
        assert_eq!(domain, &legacy);
        assert_eq!(matches!(domain, SandboxStoreError::Conflict), conflict);
        assert!(error.belongs_to(&policy));
        assert!(root.available_bytes() < idle);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            policy.bounds().data_bytes()
        );
        drop(error);
        assert_eq!(root.available_bytes(), idle);
    }
    let mut row = valid.clone();
    let mut nested = doc! {"value":true};
    for _ in 0..66 {
        nested = doc! {"nested":nested};
    }
    row.insert("unsupported", nested);
    rows.replace_one(doc! {"_id":valid.get_str("_id").unwrap()}, row.clone())
        .bypass_document_validation(true)
        .await
        .unwrap();
    assert!(matches!(
        writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            plan(),
        )
        .unwrap()
        .await,
        Err(AdmittedError::Unsupported(_))
    ));
    assert_eq!(
        rows.find_one(doc! {"_id":valid.get_str("_id").unwrap()})
            .await
            .unwrap(),
        Some(row)
    );
    assert_eq!(root.available_bytes(), idle);
    drop((session, rows, database));
    fixture.finish().await;
}

async fn observed_fixture(suffix: &str) -> (Fixture, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && event.command.get_str("find").ok() == Some(writer_admission::COLLECTION)
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    (
        Fixture::with_client(mongodb::Client::with_options(options).unwrap(), suffix).await,
        calls,
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_read_refuses_before_queries_and_preserves_data_pressure_and_cancellation() {
    let (fixture, calls) = observed_fixture("admission_refusal").await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_admission::COLLECTION);
    let operation = operation();
    let valid = bson::serialize_to_document(&admission(&fixture.agent, &operation)).unwrap();
    rows.insert_one(valid.clone()).await.unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked("occupied work", &[policy.bounds().transformation_bytes()])
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            plan(),
        ),
        Err(AdmittedError::Refused(_))
    ));
    drop(occupied);
    drop(
        writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            plan(),
        )
        .unwrap(),
    );
    assert_eq!(root.available_bytes(), idle);
    let too_large = AllocationPlan::checked("unavailable read", &[root.capacity_bytes()]).unwrap();
    assert!(matches!(
        writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            too_large,
        )
        .unwrap()
        .await,
        Err(AdmittedError::Refused(_))
    ));
    let mut invalid = operation.clone();
    invalid.key.clear();
    let error = writer_admission::read_admitted(
        &database,
        &fixture.agent,
        &invalid,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap_err();
    assert!(matches!(
        &*error,
        ReadFailure::Domain(SandboxStoreError::Corrupt(_))
    ));
    drop(error);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let occupied = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("occupied data", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        writer_admission::read_admitted(
            &database,
            &fixture.agent,
            &operation,
            &mut session,
            &policy,
            plan(),
        )
        .unwrap()
        .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(root.available_bytes(), held);
    drop(occupied);
    assert_eq!(
        bson::serialize_to_document(
            &writer_admission::read(&database, &fixture.agent, &operation, &mut session,)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        valid
    );
    assert_eq!(root.available_bytes(), idle);
    drop((session, rows, database));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_read_keeps_driver_labels_and_owned_error_storage() {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-admission-error";
    options.app_name = Some(app_name.into());
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "admission_driver",
    )
    .await;
    let database: Database = fixture.store.client.database(&fixture.database);
    let admin = fixture.store.client.database("admin");
    let operation = operation();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    begin(&mut session).await;
    admin
        .run_command(
            doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":["find"], "appName":app_name, "errorCode":112,
                "errorLabels":["TransientTransactionError"],
            }},
        )
        .await
        .unwrap();
    let result = writer_admission::read_admitted(
        &database,
        &fixture.agent,
        &operation,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let error = result.unwrap().transpose().unwrap_err();
    let ReadFailure::Driver(driver) = &*error else {
        panic!("driver classification changed")
    };
    assert!(driver.contains_label("TransientTransactionError"));
    let pointer = std::ptr::from_ref(driver.kind.as_ref());
    let moved = error.map(|error| match error {
        ReadFailure::Driver(driver) => driver,
        _ => unreachable!(),
    });
    assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
    assert!(moved.belongs_to(&policy));
    assert!(root.available_bytes() < idle);
    drop(moved);
    assert_eq!(root.available_bytes(), idle);
    let _ = session.abort_transaction().await;
    drop((session, admin, database));
    fixture.finish().await;
}

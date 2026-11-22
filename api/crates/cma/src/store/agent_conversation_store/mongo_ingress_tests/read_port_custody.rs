use super::{doc, policy, AllocationPlan, IngressClass, RawDocumentBuf, SandboxStoreError};
use crate::store::ingress_reads::{
    cursor::{DocumentSchema, Page, PageError, MAX_BYTES},
    error::{self, ReadFailure},
    layout::LayoutError,
    row::{self, RowError},
};
use layer2_core::sandbox::ingress_ownership::IngressWorkScope;

fn raw(bytes: usize) -> RawDocumentBuf {
    RawDocumentBuf::try_from(&doc! {"value":"x".repeat(bytes)}).unwrap()
}

#[test]
fn admitted_page_preserves_exact_canonical_byte_and_row_boundaries() {
    let (root, policy) = policy();
    let overhead = raw(0).as_bytes().len();
    let exact = raw(MAX_BYTES - overhead);
    let mut page = Page::new(&policy, 1).unwrap();
    page.push(&exact, &policy).unwrap();
    let result = page.finish();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].get_str("value").unwrap().len(),
        MAX_BYTES - overhead
    );
    drop(result);
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(MAX_BYTES / 2 - overhead), &policy).unwrap();
    page.push(&raw(MAX_BYTES / 2 - overhead), &policy).unwrap();
    assert_eq!(page.finish().len(), 2);
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(MAX_BYTES / 2), &policy).unwrap();
    assert!(matches!(
        page.push(&raw(MAX_BYTES / 2), &policy),
        Err(PageError::Invalid(
            "transaction page exceeds its byte budget"
        ))
    ));
    drop(page);
    let mut page = Page::new(&policy, 256).unwrap();
    let tiny = raw(0);
    for _ in 0..256 {
        page.push(&tiny, &policy).unwrap();
    }
    assert!(matches!(
        page.push(&tiny, &policy),
        Err(PageError::Invalid(
            "transaction page exceeds its row budget"
        ))
    ));
    drop(page);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn page_accounts_canonical_documents_with_repeated_bson_keys() {
    let (root, policy) = policy();
    // A repeated field collapses during Document conversion. The legacy page
    // limit counts that canonical representation, not the original raw bytes.
    let original = raw(MAX_BYTES / 2);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&original.as_bytes()[4..original.as_bytes().len() - 1]);
    bytes.extend_from_slice(&raw(4).as_bytes()[4..]);
    let len = bytes.len() as i32;
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    let duplicate = RawDocumentBuf::from_bytes(bytes).unwrap();
    let mut page = Page::new(&policy, 3).unwrap();
    for _ in 0..3 {
        page.push(&duplicate, &policy).unwrap();
    }
    let values = page.finish();
    assert_eq!(values.len(), 3);
    assert!(values
        .iter()
        .all(|value| value.get_str("value").unwrap() == "xxxx"));
    drop(values);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn page_refusal_preserves_prior_rows_and_independent_data_custody() {
    let (root, policy) = policy();
    let mut page = Page::new(&policy, 2).unwrap();
    page.push(&raw(8192), &policy).unwrap();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("remaining Data", &[available]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        page.push(&raw(32), &policy),
        Err(PageError::Row(RowError::Refused(LayoutError::Capacity)))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let values = page.finish();
    let first = values[0].clone();
    let held = root.available_bytes();
    drop(values);
    assert!(root.available_bytes() > held);
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(first.get_str("value").unwrap().len(), 8192);
    drop(first);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test]
async fn source_error_preserves_its_nonclone_payload_through_operation_composition() {
    let (root, policy) = policy();
    let address = std::cell::Cell::new(std::ptr::null());
    let result = row::load_with_error::<DocumentSchema, _, _>(
        &policy,
        AllocationPlan::checked("read error", &[64 * 1024]).unwrap(),
        || {
            let error = mongodb::error::Error::custom("synthetic".repeat(1024));
            address.set(std::ptr::from_ref(error.kind.as_ref()));
            std::future::ready(Err(error))
        },
    )
    .await;
    let storage = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("result metadata", &[1024]).unwrap(),
            )
            .unwrap(),
    );
    let error = error::finish(result, &storage)
        .unwrap()
        .transpose()
        .unwrap_err();
    drop(storage);
    let ReadFailure::Driver(driver) = &*error else {
        panic!("driver error was erased")
    };
    assert_eq!(std::ptr::from_ref(driver.kind.as_ref()), address.get());
    assert!(driver
        .get_custom::<String>()
        .unwrap()
        .starts_with("synthetic"));
    assert!(error.belongs_to(&policy));
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    drop(error);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[test]
fn stored_corruption_and_allocation_refusal_have_different_error_channels() {
    let (root, policy) = policy();
    let storage = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("result metadata", &[1024]).unwrap(),
            )
            .unwrap(),
    );
    let malformed = RawDocumentBuf::from_bytes(vec![8, 0, 0, 0, 2, b'x', 0, 0]).unwrap();
    let result = row::decode::<DocumentSchema>(&malformed, &policy).map_err(row::ReadError::Row);
    assert!(matches!(
        error::finish(result, &storage),
        Err(layer2_core::sandbox::ingress_ownership::AdmittedError::Unsupported(_))
    ));
    let diagnostic = storage
        .initialize_owned(|| SandboxStoreError::Corrupt("synthetic schema violation".into()));
    let result: Result<(), _> = Err(row::ReadError::Row(RowError::Domain(diagnostic)));
    let error = error::finish(result, &storage)
        .unwrap()
        .transpose()
        .unwrap_err();
    assert!(
        matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message))
        if message == "synthetic schema violation")
    );
    drop(error);
    let result: Result<(), _> = Err(row::ReadError::Row(RowError::Refused(
        LayoutError::Capacity,
    )));
    assert!(matches!(
        error::finish(result, &storage),
        Err(layer2_core::sandbox::ingress_ownership::AdmittedError::Refused(_))
    ));
    drop(storage);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

use super::fixture::Fixture;
use crate::store::agent_log_segments::writer_gate::{self, GateName, GateRecord, Policy};
use layer2_core::sandbox::ingress_ownership::AdmittedError;
use mongodb::bson::{self, Document};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn gate_plan() -> AllocationPlan {
    AllocationPlan::checked("gate read custody", &[50 * 1024 * 1024]).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn gate_read_refuses_before_queries_and_releases_cancelled_and_data_refused_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && event.command.get_str("find").ok() == Some(writer_gate::COLLECTION)
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "gate_refusal",
    )
    .await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_gate::COLLECTION);
    let name = GateName::Segments;
    let valid = bson::serialize_to_document(&GateRecord {
        name,
        format_version: 1,
        revision: 1,
        admission_fence: 0,
        policy: Policy::default(),
        slots: Vec::new(),
    })
    .unwrap();
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
        writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan()),
        Err(AdmittedError::Refused(_))
    ));
    drop(occupied);
    drop(writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan()).unwrap());
    assert_eq!(root.available_bytes(), idle);
    let oversized = AllocationPlan::checked("unavailable read", &[root.capacity_bytes()]).unwrap();
    assert!(matches!(
        writer_gate::read_admitted(&database, name, &mut session, &policy, oversized)
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let occupied = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("occupied data", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(root.available_bytes(), held);
    drop(occupied);
    assert_eq!(
        rows.find_one(doc! {"_id":name.as_str()}).await.unwrap(),
        Some(valid)
    );
    assert_eq!(root.available_bytes(), idle);
    drop((session, rows, database));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn gate_read_keeps_driver_labels_and_error_allocation_identity() {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-gate-error";
    options.app_name = Some(app_name.into());
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "gate_driver",
    )
    .await;
    let database = fixture.store.client.database(&fixture.database);
    let admin = fixture.store.client.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    admin
        .run_command(
            doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":["find"], "appName":app_name, "errorCode":112,
                "errorLabels":["TransientTransactionError"],
            }},
        )
        .await
        .unwrap();
    let result = writer_gate::read_admitted(
        &database,
        GateName::Segments,
        &mut session,
        &policy,
        gate_plan(),
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn seal_read_admission_and_unpolled_cancellation_issue_no_queries() {
    use crate::store::agent_log_segments::{Seal, SEGMENTS};
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command_name == "find" && event.command.get_str("find").ok() == Some(SEGMENTS)
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "seal_refusal",
    )
    .await;
    let store = &fixture.store;
    let (head, original) = super::read_ports::seal_fixture(1);
    store
        .segment_logs
        .seals
        .insert_one(bson::serialize_to_document(&original).unwrap())
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked("occupied work", &[policy.bounds().transformation_bytes()])
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, gate_plan()),
        Err(AdmittedError::Refused(_))
    ));
    drop(occupied);
    drop(
        Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, gate_plan()).unwrap(),
    );
    let plan = AllocationPlan::checked("unavailable read", &[root.capacity_bytes()]).unwrap();
    assert!(matches!(
        Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, plan)
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), idle);
    let result = Seal::read_at_admitted(store, &head, 0, &mut session, false, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(**result, original);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop((result, session));
    assert_eq!(root.available_bytes(), idle);
    fixture.finish().await;
}

use super::{doc, fixture::Fixture, policy, AllocationPlan, IngressClass};
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use crate::store::ingress_reads::cursor::DocumentSchema;
use layer2_core::sandbox::ingress_ownership::{AdmissionRefusal, AdmittedError};
use mongodb::bson::Document;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn a_new_transaction_sizes_the_replaced_row_without_reusing_earlier_admission() {
    let fixture = Fixture::new("read_replacement").await;
    let rows = fixture
        .store
        .client
        .database(&fixture.database)
        .collection::<Document>("ingress_rows");
    rows.insert_one(doc! {"_id":"row", "value":"small"})
        .await
        .unwrap();
    let (root, policy) = policy();
    let plan = || AllocationPlan::checked("replacement read", &[50 * 1024 * 1024]).unwrap();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let first = transaction
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(first.as_ref().unwrap().get_str("value").unwrap(), "small");
    drop(first);
    let other = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    other
        .database(&fixture.database)
        .collection::<Document>("ingress_rows")
        .update_one(
            doc! {"_id":"row"},
            doc! {"$set":{"value":"x".repeat(256 * 1024)}},
        )
        .await
        .unwrap();
    let prior = transaction
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(prior.as_ref().unwrap().get_str("value").unwrap(), "small");
    drop(prior);
    transaction.release().abort_transaction().await.unwrap();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "limited returned row storage",
                &[policy.bounds().data_bytes() - 8192],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    let mut later = TransactionScope::start(&mut session).await.unwrap();
    let result = later
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await;
    assert!(matches!(
        result,
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let current = later
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(
        current.as_ref().unwrap().get_str("value").unwrap().len(),
        256 * 1024
    );
    drop(current);
    later.release().abort_transaction().await.unwrap();
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    drop((session, rows));
    other.shutdown().await;
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_and_legacy_reads_use_the_same_ordinary_work_schema_predicate() {
    let fixture = Fixture::new("read_schema").await;
    let (root, policy) = policy();
    let plan = || AllocationPlan::checked("schema predicate read", &[50 * 1024 * 1024]).unwrap();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let filter = || doc! {"$and":[{"_id":fixture.agent.sandbox_id.as_str()}, {"document_schema_version":1_i64}]};
    let legacy = transaction
        .find_one(&fixture.store.sandboxes, filter())
        .await
        .unwrap()
        .unwrap();
    let row = transaction
        .find_one_admitted::<DocumentSchema, _>(&fixture.store.sandboxes, &policy, plan(), filter)
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(&**row.as_ref().unwrap(), &legacy);
    drop(row);
    let page = transaction
        .find_page_admitted(&fixture.store.sandboxes, &policy, plan(), 2, || {
            (filter(), doc! {"_id":1})
        })
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(*page[0], legacy);
    drop(page);
    transaction.release().abort_transaction().await.unwrap();
    drop(session);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

use crate::store::agent_log_segments::writer_gate::{self, GateName, GateRecord, Policy};
use crate::store::{agent_log_integrity::TransactionLogError, ingress_reads::error::ReadFailure};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{self, Bson};

fn gate_row(name: GateName, count: usize) -> Document {
    bson::serialize_to_document(&GateRecord {
        name,
        format_version: 1,
        revision: 1,
        admission_fence: 0,
        policy: Policy {
            enabled: true,
            agents: (0..count).map(|n| format!("{n:0512}")).collect(),
            ..Policy::default()
        },
        slots: Vec::new(),
    })
    .unwrap()
}

fn gate_plan() -> AllocationPlan {
    AllocationPlan::checked("gate read", &[50 * 1024 * 1024]).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn gate_read_preserves_both_names_own_writes_rollback_and_independent_retention() {
    let fixture = Fixture::new("gate_retention").await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_gate::COLLECTION);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    for name in [GateName::Segments, GateName::CapacityRecovery] {
        let original = gate_row(name, 1);
        rows.insert_one(original.clone()).await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let legacy = writer_gate::read(&database, name, &mut session)
            .await
            .unwrap()
            .unwrap();
        let result =
            writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap();
        let retained = result.as_ref().unwrap().clone();
        assert_eq!(*retained, legacy);
        drop(result);
        assert!(retained.belongs_to(&policy));
        assert_eq!(
            policy
                .budget(IngressClass::Transformation)
                .available_bytes(),
            policy.bounds().transformation_bytes()
        );
        let available = policy.budget(IngressClass::Data).available_bytes();
        rows.replace_one(doc! {"_id":name.as_str()}, gate_row(name, 64))
            .session(&mut session)
            .await
            .unwrap();
        let grown = writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(grown.as_ref().unwrap().policy.agents.len(), 64);
        assert_eq!(retained.policy.agents.len(), 1);
        assert!(available - policy.budget(IngressClass::Data).available_bytes() >= 64 * 512);
        session.abort_transaction().await.unwrap();
        drop(grown);
        assert_eq!(
            policy.budget(IngressClass::Data).available_bytes(),
            available
        );
        let later = writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(**later.as_ref().unwrap(), legacy);
        drop((later, retained));
        assert_eq!(root.available_bytes(), idle);
        assert_eq!(
            rows.find_one(doc! {"_id":name.as_str()}).await.unwrap(),
            Some(original)
        );
    }
    drop((session, rows, database));
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn gate_read_keeps_absence_strict_validation_and_preflight_refusal_distinct() {
    let fixture = Fixture::new("gate_validation").await;
    let database = fixture.store.client.database(&fixture.database);
    let rows = database.collection::<Document>(writer_gate::COLLECTION);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = fixture.store.client.start_session().await.unwrap();
    for name in [GateName::Segments, GateName::CapacityRecovery] {
        let absent =
            writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap();
        assert!(absent.is_none());
        drop(absent);
        let valid = gate_row(name, 1);
        for (field, replacement) in [
            ("format_version", Bson::Int64(2)),
            ("revision", Bson::Int32(1)),
            ("admission_fence", Bson::Int64(-1)),
            (
                "policy.agents",
                bson::serialize_to_bson(&vec!["duplicate"; 2]).unwrap(),
            ),
            (
                "policy.agents",
                bson::serialize_to_bson(&vec!["x"; 65]).unwrap(),
            ),
            (
                "slots",
                bson::serialize_to_bson(&vec![doc! {"holder":"missing-fields"}]).unwrap(),
            ),
            ("unexpected", Bson::String("x".repeat(64 * 1024))),
        ] {
            let mut row = valid.clone();
            if field == "policy.agents" {
                row.get_document_mut("policy")
                    .unwrap()
                    .insert("agents", replacement);
            } else {
                row.insert(field, replacement);
            }
            rows.replace_one(doc! {"_id":name.as_str()}, row.clone())
                .upsert(true)
                .await
                .unwrap();
            let TransactionLogError::Domain(legacy) =
                writer_gate::read(&database, name, &mut session)
                    .await
                    .unwrap_err()
            else {
                panic!("domain became driver")
            };
            let error =
                writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
                    .unwrap()
                    .await
                    .unwrap()
                    .transpose()
                    .unwrap_err();
            let ReadFailure::Domain(domain) = &*error else {
                panic!("domain became driver/refusal")
            };
            assert_eq!(domain, &legacy);
            assert!(matches!(domain, SandboxStoreError::Corrupt(_)));
            assert!(error.belongs_to(&policy));
            assert!(root.available_bytes() < idle);
            assert_eq!(
                policy.budget(IngressClass::Data).available_bytes(),
                policy.bounds().data_bytes()
            );
            drop(error);
            assert_eq!(root.available_bytes(), idle);
            assert_eq!(
                rows.find_one(doc! {"_id":name.as_str()}).await.unwrap(),
                Some(row)
            );
        }
        let mut row = valid;
        let mut nested = doc! {"value":true};
        for _ in 0..66 {
            nested = doc! {"nested":nested};
        }
        row.insert("unsupported", nested);
        rows.replace_one(doc! {"_id":name.as_str()}, row.clone())
            .await
            .unwrap();
        assert!(matches!(
            writer_gate::read_admitted(&database, name, &mut session, &policy, gate_plan())
                .unwrap()
                .await,
            Err(AdmittedError::Unsupported(_))
        ));
        assert_eq!(
            rows.find_one(doc! {"_id":name.as_str()}).await.unwrap(),
            Some(row)
        );
        assert_eq!(root.available_bytes(), idle);
    }
    drop((session, rows, database));
    fixture.finish().await;
}

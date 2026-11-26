//! Selector keysets and permanent cohort-level refusals on a real store.
use super::{tests::*, *};
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unchanged_indexed_history_resumes_inside_the_personal_selector() {
    let db = database().await;
    let logs = db.collection::<Document>("agent_log");
    logs.insert_many((0..1201).map(|n| {
        doc! {
            "_id":format!("owned-{n:05}"),"agent_id":"owned-agent","seq":n
        }
    }))
    .await
    .unwrap();
    logs.insert_many((0..10000).map(|n| {
        doc! {
            "_id":format!("foreign-{n:05}"),"agent_id":"foreign-agent","seq":n
        }
    }))
    .await
    .unwrap();
    indexes::ensure(&db).await.unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let mut contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let context = contexts.pop().unwrap();
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_log")
        .unwrap();
    let first = batches::select(&db, entry, true, std::slice::from_ref(&context), None)
        .await
        .unwrap();
    assert_eq!(first.len(), 500);
    let after = first.last().unwrap().get("_id").unwrap();
    let second = batches::select(&db, entry, true, &[context], Some(after))
        .await
        .unwrap();
    assert_eq!(second.len(), 500);
    let first_ids: BTreeSet<_> = first
        .iter()
        .map(|row| row.get_str("_id").unwrap())
        .collect();
    assert!(
        second
            .iter()
            .all(|row| !first_ids.contains(row.get_str("_id").unwrap())),
        "an unchanged page must advance without a collection-wide fallback"
    );
    assert!(second
        .iter()
        .all(|row| row.get_str("agent_id").unwrap() == "owned-agent"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unattributable_gate_corruption_permanently_blocks_the_cohort_and_repair_resumes() {
    let db = database().await;
    put(
        &db,
        "agent_log_gates",
        doc! {
            "_id":"unknown-gate","policy":{"agents":["owned-agent"]}
        },
    )
    .await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    let mut cursor = Cursor::default();
    let mut failure = None;
    for _ in 0..500 {
        match erase_records(&db, &targets, &mut cursor, NOW).await {
            Ok(batch) => assert!(batch.deleted.is_empty() && batch.redacted.is_empty()),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    let failure = failure.expect("a malformed matching global gate must stop erasure");
    assert!(
        matches!(
            failure,
            Error::CohortBlocked(CohortBlock::CorruptRecord {
                collection: "agent_log_gates"
            })
        ),
        "deterministic damage without owner evidence must be permanent: {failure:?}"
    );
    assert_eq!(count(&db, "agent_log_gates").await, 1);
    db.collection::<Document>("agent_log_gates")
        .delete_many(doc! {})
        .await
        .unwrap();
    drain(&db, "a", &mut inventory.clone()).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn observed_lifetime_above_qualification_is_a_permanent_cohort_block() {
    let db = database().await;
    let admin = db.client().database("admin");
    let previous = admin
        .run_command(doc! {"getParameter":1,"transactionLifetimeLimitSeconds":1})
        .await
        .unwrap();
    let lifetime = previous
        .get_i32("transactionLifetimeLimitSeconds")
        .map(i64::from)
        .or_else(|_| previous.get_i64("transactionLifetimeLimitSeconds"))
        .unwrap() as u64;
    assert!(lifetime > 1);
    let qualification =
        TransactionDrainQualification::from_verified_limits([2; 32], lifetime - 1, lifetime * 2)
            .unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let result = barrier::ready(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(qualification),
            billing_enabled: false,
            transaction_drain_minimum_secs: 120,
        },
        &mut Batch::default(),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(Error::CohortBlocked(
                CohortBlock::InvalidTransactionDrain { .. }
            ))
        ),
        "a lifetime above the qualified bound must require operator intervention: {result:?}"
    );
    assert!(cursor.drain_until.is_none());
    assert!(barrier::ready(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory
        }],
        &mut cursor,
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: 120
        },
        &mut Batch::default()
    )
    .await
    .is_ok());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn absurd_configured_drain_is_permanent_instead_of_an_infinite_retry() {
    let db = database().await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let result = barrier::ready(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: u64::MAX,
        },
        &mut Batch::default(),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(Error::CohortBlocked(
                CohortBlock::InvalidTransactionDrain { .. }
            ))
        ),
        "an impossible configured deadline requires operator intervention: {result:?}"
    );
    assert!(cursor.drain_until.is_none());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn permanent_cohort_corruption_returns_the_completed_redaction_count() {
    let db = database().await;
    put(
        &db,
        "agent_log_gates",
        doc! {
            "_id":"agent_log_segments_v1","format_version":1_i64,"revision":1_i64,
            "admission_fence":0_i64,"policy":{"enabled":true,"owners":[],
                "agents":["owned-agent"],"cohort_seed":"","basis_points":0_i32},"slots":[]
        },
    )
    .await;
    put(
        &db,
        "agent_log_gates",
        doc! {"_id":"z-invalid-gate","policy":{"agents":["owned-agent"]}},
    )
    .await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    let mut cursor = Cursor::default();
    let mut interrupted = false;
    for _ in 0..500 {
        match erase_records(&db, &targets, &mut cursor, NOW).await {
            Err(Error::Interrupted { error, batch }) => {
                assert!(matches!(
                    *error,
                    Error::CohortBlocked(CohortBlock::CorruptRecord {
                        collection: "agent_log_gates"
                    })
                ));
                assert_eq!(batch.redacted.get("agent_log_gates"), Some(&1));
                assert!(batch.waiting && batch.has_more);
                interrupted = true;
                break;
            }
            Ok(batch) => assert!(batch.redacted.is_empty() && batch.deleted.is_empty()),
            other => panic!("expected completed progress with the permanent cause: {other:?}"),
        }
    }
    assert!(interrupted);
    let gate = db
        .collection::<Document>("agent_log_gates")
        .find_one(doc! {"_id":"agent_log_segments_v1"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(gate.get_i64("revision").unwrap(), 2);
    assert!(gate
        .get_document("policy")
        .unwrap()
        .get_array("agents")
        .unwrap()
        .is_empty());
    db.collection::<Document>("agent_log_gates")
        .delete_one(doc! {"_id":"z-invalid-gate"})
        .await
        .unwrap();
    drain(&db, "a", &mut inventory.clone()).await;
    assert_eq!(count(&db, "agent_log_gates").await, 1);
    db.drop().await.unwrap();
}

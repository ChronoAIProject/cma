//! Actual restricted credentials, command receipts, and namespace effects.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::{ClientOptions, Credential},
    Client,
};
use std::sync::{Arc, Mutex};

const OPTIONAL: &str = crate::store::allocation_buckets::COLLECTION;

#[path = "optional_authority_fixture.rs"]
mod fixture;
use fixture::Fixture;
#[path = "observer_authority_tests.rs"]
mod observers;

pub(crate) async fn verify_runtime() {
    retained_consent_census_works_without_read_authority().await;
    missing_drain_qualification().await;
    observers::verify().await;
    disabled_absent().await;
    present_without_grants().await;
    read_without_cleanup().await;
    present_with_exact_grants().await;
    missing_metadata().await;
    missing_mandatory().await;
    enabled_or_unknown().await;
    resumed_absence_is_rechecked().await;
    resumed_appearance_restarts_effects().await;
}

async fn retained_consent_census_works_without_read_authority() {
    use crate::store::retained_consents::{self, COLLECTION};
    for enabled in [false, true] {
        let fixture = Fixture::for_collection(COLLECTION, enabled, None, None).await;
        assert!(fixture
            .runtime
            .collection::<Document>(COLLECTION)
            .find_one(doc! {})
            .await
            .is_err());
        if enabled {
            super::legal_consent_tests::user(&fixture.admin, "acct:retained").await;
            retained_consents::prepare(&fixture.runtime).await.unwrap();
            assert!(
                retained_consents::erase_user(&fixture.runtime, "acct:retained")
                    .await
                    .unwrap()
            );
        }
        fixture.commands.lock().unwrap().clear();
        crate::store::retirement_archive::verify_legal_consent_census(
            &fixture.admin,
            &fixture.runtime,
        )
        .await;
        assert!(!fixture.commands.lock().unwrap().iter().any(|command| {
            ["find", "aggregate", "count"]
                .iter()
                .any(|name| command.get_str(*name).ok() == Some(COLLECTION))
        }));
        assert_eq!(count(&fixture.admin, COLLECTION).await, u64::from(enabled));
        fixture.admin.drop().await.unwrap();
    }
}

async fn missing_drain_qualification() {
    let fixture = Fixture::new(false, None).await;
    put(
        &fixture.admin,
        "users",
        doc! {"_id":"acct:a","canary":"preserve"},
    )
    .await;
    let subject = SubjectId::new("acct:a");
    let error = super::erase_records_with_options(
        &fixture.runtime,
        &[Target {
            subject: &subject,
            inventory: &ErasureInventory::default(),
        }],
        &mut Cursor {
            drain_complete: true,
            ..Default::default()
        },
        NOW,
        Options {
            billing_enabled: false,
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            transaction_drain_minimum_secs: 86_400,
            drain_qualification: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        Error::CohortBlocked(CohortBlock::TransactionDrainQualificationRequired)
    ));
    assert_eq!(count(&fixture.admin, "users").await, 1);
    {
        let commands = fixture.commands.lock().unwrap();
        for command in commands.iter() {
            for operation in [
                "create",
                "createIndexes",
                "update",
                "insert",
                "delete",
                "findAndModify",
            ] {
                assert!(!command.contains_key(operation));
            }
        }
    }
    fixture.admin.drop().await.unwrap();
}

async fn disabled_absent() {
    let fixture = Fixture::new(false, None).await;
    crate::store::isolation::replicas::verify(
        &fixture.runtime,
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
        true,
    )
    .await
    .unwrap();
    put(&fixture.admin, "users", doc! {"_id":"acct:a"}).await;
    put(&fixture.admin, "users", doc! {"_id":"acct:b"}).await;
    fixture.absent().await;
    let deleted = drain(&fixture.runtime, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted.get("users"), Some(&1));
    assert_eq!(count(&fixture.admin, "users").await, 1);
    fixture.absent().await;
    {
        let commands = fixture.commands.lock().unwrap();
        for command in commands.iter() {
            for operation in [
                "createIndexes",
                "create",
                "find",
                "delete",
                "aggregate",
                "listIndexes",
            ] {
                assert_ne!(
                    command.get_str(operation).ok(),
                    Some(OPTIONAL),
                    "disabled absent namespace received {operation}"
                );
            }
        }
        assert!(commands
            .iter()
            .any(|command| command.get_i32("listCollections").ok() == Some(1)
                && command.get_bool("authorizedCollections").ok() == Some(false)));
    }
    println!("optional-authority: disabled absent namespace remained absent; real erasure progressed without optional grants");
    fixture.admin.drop().await.unwrap();
}

fn bucket(sandbox: &str, owner: &str) -> Document {
    use layer2_core::{
        sandbox_metrics::Range,
        sandbox_usage::{Allocation, SandboxUsage, UsageTotals},
        SandboxId,
    };
    let from = 100 * 86_400;
    let state = SandboxUsage {
        sandbox_id: SandboxId(sandbox.into()),
        owner: SubjectId::new(owner),
        name: "Authority fixture".into(),
        status: "retired".into(),
        allocation: Allocation {
            running: false,
            cpu_millis: 1000,
            memory_mib: 1024,
            disk_gib: 8,
        },
        created_at: from,
        tracking_started_at: from,
        last_observed_at: from + 86_400,
        source_seq: 1,
        source_revision: 1,
        revision: 1,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    };
    crate::store::usage_reports::ReportingStore::allocation_bucket_fixture_document(
        &state,
        &[],
        Range {
            from,
            to: from + 86_400,
        },
    )
    .unwrap()
}

async fn seed_buckets(db: &Database) -> Vec<Document> {
    sandbox(db, "acct:a", "sbx_a").await;
    sandbox(db, "acct:b", "sbx_b").await;
    let rows = vec![
        bucket("sbx_a", "acct:a"),
        bucket("sbx_gone", "acct:a"),
        bucket("sbx_b", "acct:b"),
    ];
    for row in &rows {
        put(db, OPTIONAL, row.clone()).await;
    }
    rows
}

async fn once_with(
    fixture: &Fixture,
    mode: Option<bool>,
    cursor: &mut Cursor,
) -> Result<Batch, Error> {
    let subject = SubjectId::new("acct:a");
    let inventory = ErasureInventory::default();
    cursor.test_drain_seconds.get_or_insert(0);
    super::erase_records_with_options(
        &fixture.runtime,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        cursor,
        NOW,
        Options {
            transaction_drain_minimum_secs: 120,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            allocation_rollups_enabled: mode,
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
        },
    )
    .await
}

fn authority_block(error: Error, expected: &str) {
    assert!(
        matches!(error, Error::CohortBlocked(CohortBlock::CollectionAuthority {
        collection: OPTIONAL, operation,
    }) if operation == expected),
        "expected explicit {expected} authority refusal: {error:?}"
    );
}

async fn present_without_grants() {
    let fixture = Fixture::new(false, None).await;
    let rows = seed_buckets(&fixture.admin).await;
    authority_block(
        once_with(&fixture, Some(false), &mut Cursor::default())
            .await
            .unwrap_err(),
        "find",
    );
    for row in rows {
        assert_eq!(
            fixture
                .admin
                .collection::<Document>(OPTIONAL)
                .find_one(doc! {"_id":row.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(row)
        );
    }
    assert_eq!(count(&fixture.admin, "sandboxes").await, 2);
    println!("optional-authority: existing namespace without grants blocked; linked, orphan-owner and other-tenant writer rows unchanged");
    fixture.admin.drop().await.unwrap();
}

async fn present_with_exact_grants() {
    let fixture = Fixture::new(true, None).await;
    crate::store::isolation::replicas::verify(
        &fixture.runtime,
        cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
        true,
    )
    .await
    .unwrap();
    let rows = seed_buckets(&fixture.admin).await;
    let retained = [
        (
            "cma_agent_manager_owners",
            doc! {"_id":"agt_a","instance_id":"manager-a","epoch":19_i64},
        ),
        (
            "cma_agent_manager_members",
            doc! {"_id":"manager-a","protocol":1_i64,"draining":false},
        ),
        (
            "sandbox_credential_issuance",
            doc! {"_id":"sbx_a","document_schema_version":1_i64,"record":{"owner":"acct:a","state":"unknown"}},
        ),
        (
            "sandbox_usage",
            doc! {"_id":"sbx_b","owner":"acct:b","revision":31_i64},
        ),
    ];
    for (name, row) in &retained {
        put(&fixture.admin, name, row.clone()).await;
    }
    let deleted = drain(&fixture.runtime, "acct:a", &mut ErasureInventory::default()).await;
    assert_eq!(deleted.get(OPTIONAL), Some(&2));
    assert_eq!(count(&fixture.admin, OPTIONAL).await, 1);
    assert_eq!(
        fixture
            .admin
            .collection::<Document>(OPTIONAL)
            .find_one(doc! {"_id":rows[2].get("_id").unwrap().clone()})
            .await
            .unwrap(),
        Some(rows[2].clone())
    );
    for (name, row) in retained {
        assert_eq!(
            fixture
                .admin
                .collection::<Document>(name)
                .find_one(doc! {"_id":row.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(row)
        );
    }
    {
        let commands = fixture.commands.lock().unwrap();
        assert!(commands
            .iter()
            .any(|c| c.get_str("find").ok() == Some(OPTIONAL)
                && c.get_document("filter")
                    .is_ok_and(|f| f.contains_key("sandbox_id"))));
        assert!(commands
            .iter()
            .any(|c| c.get_str("find").ok() == Some(OPTIONAL)
                && c.get_document("filter")
                    .is_ok_and(|f| !f.contains_key("sandbox_id") && !f.contains_key("_id"))));
    }
    println!("optional-authority: exact grants erased both indexed-linked and scanned-owner rows; survivor, permanent epoch, member, credential custody and aggregate authority retained");
    fixture.admin.drop().await.unwrap();
}

async fn read_without_cleanup() {
    let fixture = Fixture::with_missing_action(true, None, Some("remove")).await;
    seed_buckets(&fixture.admin).await;
    authority_block(
        once_with(&fixture, Some(false), &mut Cursor::default())
            .await
            .unwrap_err(),
        "remove",
    );
    assert_eq!(count(&fixture.admin, OPTIONAL).await, 3);
    assert_eq!(count(&fixture.admin, "sandboxes").await, 2);
    println!(
        "optional-authority: read authority alone did not authorize cleanup or report zero residue"
    );
    fixture.admin.drop().await.unwrap();
}

async fn missing_metadata() {
    let fixture = Fixture::new(false, Some("")).await;
    put(&fixture.admin, "users", doc! {"_id":"acct:a"}).await;
    // Authorized-only metadata really does hide a present inaccessible namespace.
    put(&fixture.admin, OPTIONAL, bucket("sbx_gone", "acct:a")).await;
    let visible = fixture
        .runtime
        .list_collection_names()
        .authorized_collections(true)
        .await
        .unwrap();
    assert!(!visible.iter().any(|name| name == OPTIONAL));
    authority_block(
        once_with(&fixture, Some(false), &mut Cursor::default())
            .await
            .unwrap_err(),
        "listCollections",
    );
    assert_eq!(count(&fixture.admin, OPTIONAL).await, 1);
    fixture
        .admin
        .collection::<Document>(OPTIONAL)
        .drop()
        .await
        .unwrap();
    authority_block(
        once_with(&fixture, Some(false), &mut Cursor::default())
            .await
            .unwrap_err(),
        "listCollections",
    );
    fixture.absent().await;
    assert_eq!(count(&fixture.admin, "users").await, 1);
    println!("optional-authority: authorized-only invisibility is not absence; missing complete metadata blocked for present and absent namespaces");
    fixture.admin.drop().await.unwrap();
}

async fn missing_mandatory() {
    let fixture = Fixture::new(false, Some("sandboxes")).await;
    put(&fixture.admin, "users", doc! {"_id":"acct:a"}).await;
    assert!(once_with(&fixture, Some(false), &mut Cursor::default())
        .await
        .is_err());
    assert_eq!(count(&fixture.admin, "users").await, 1);
    assert!(fixture
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.get_str("createIndexes").ok() == Some("sandboxes")));
    fixture.absent().await;
    println!("optional-authority: absent mandatory namespace still required index authority and failed closed");
    fixture.admin.drop().await.unwrap();
}

async fn enabled_or_unknown() {
    for mode in [Some(true), None] {
        let fixture = Fixture::new(false, None).await;
        authority_block(
            once_with(&fixture, mode, &mut Cursor::default())
                .await
                .unwrap_err(),
            "find",
        );
        fixture.absent().await;
        fixture.admin.drop().await.unwrap();
        let fixture = Fixture::new(true, None).await;
        assert!(
            once_with(&fixture, mode, &mut Cursor::default())
                .await
                .unwrap()
                .has_more
        );
        assert!(fixture
            .admin
            .list_collection_names()
            .await
            .unwrap()
            .iter()
            .any(|name| name == OPTIONAL));
        fixture.admin.drop().await.unwrap();
    }
    println!("optional-authority: enabled and unknown modes required real authority even before namespace creation");
}

async fn resumed_absence_is_rechecked() {
    let fixture = Fixture::new(false, None).await;
    put(&fixture.admin, "users", doc! {"_id":"acct:a"}).await;
    let mut cursor = Cursor::default();
    assert!(
        once_with(&fixture, Some(false), &mut cursor)
            .await
            .unwrap()
            .has_more
    );
    fixture.absent().await;
    let serialized = mongodb::bson::serialize_to_document(&cursor).unwrap();
    cursor = mongodb::bson::deserialize_from_document(serialized).unwrap();
    put(&fixture.admin, OPTIONAL, bucket("sbx_gone", "acct:a")).await;
    authority_block(
        once_with(&fixture, Some(false), &mut cursor)
            .await
            .unwrap_err(),
        "find",
    );
    assert_eq!(count(&fixture.admin, OPTIONAL).await, 1);
    assert_eq!(count(&fixture.admin, "users").await, 1);
    println!("optional-authority: serialized cursor absence never authorized a later batch after namespace appearance");
    fixture.admin.drop().await.unwrap();
}

async fn resumed_appearance_restarts_effects() {
    let fixture = Fixture::new(true, None).await;
    let mut cursor = Cursor::default();
    assert!(
        once_with(&fixture, Some(false), &mut cursor)
            .await
            .unwrap()
            .has_more
    );
    fixture.absent().await;
    let operations = super::progress::operations();
    cursor.operation = operations.len();
    super::progress::bind(&mut cursor, &operations);
    cursor = mongodb::bson::deserialize_from_document(
        mongodb::bson::serialize_to_document(&cursor).unwrap(),
    )
    .unwrap();
    put(&fixture.admin, OPTIONAL, bucket("sbx_gone", "acct:a")).await;
    let mut deleted = 0;
    let mut finished = false;
    for _ in 0..2000 {
        let batch = once_with(&fixture, Some(false), &mut cursor).await.unwrap();
        deleted += batch.deleted.get(OPTIONAL).copied().unwrap_or(0);
        if !batch.has_more {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(deleted, 1);
    assert_eq!(count(&fixture.admin, OPTIONAL).await, 0);
    println!("optional-authority: newly present namespace restarted completed keysets and installed required indexes before cleanup");
    fixture.admin.drop().await.unwrap();
}

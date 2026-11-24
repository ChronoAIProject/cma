//! Server lifetime cleanup and configured operational drain floors.
use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client,
};

async fn deadline(unreadable: bool, configured: u64) {
    let seed = seeded("round3_drain").await;
    let admin = seed.client.database("admin");
    let parameter = admin
        .run_command(doc! {"getParameter":1,"transactionLifetimeLimitSeconds":1})
        .await
        .unwrap();
    let limit = parameter
        .get_i32("transactionLifetimeLimitSeconds")
        .map(i64::from)
        .or_else(|_| parameter.get_i64("transactionLifetimeLimitSeconds"))
        .unwrap() as u64;
    let qualification =
        TransactionDrainQualification::from_verified_limits([2; 32], limit, limit * 2).unwrap();
    let expected = (limit * 2).max(configured);
    let mut settings = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    settings.app_name = Some(seed.database.clone());
    let client = Client::with_options(settings).unwrap();
    let db = client.database(&seed.database);
    if unreadable {
        admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["getParameter"],"appName":&seed.database,"errorCode":13}}).await.unwrap();
    }
    let before = *db
        .run_command(doc! {"hello":1})
        .await
        .unwrap()
        .get_datetime("localTime")
        .unwrap();
    let mut cursor = Cursor::default();
    let mut batch = Batch::default();
    let inventory = ErasureInventory::default();
    let result = super::barrier::ready(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut cursor,
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(qualification),
            billing_enabled: false,
            transaction_drain_minimum_secs: configured,
        },
        &mut batch,
    )
    .await;
    if unreadable {
        admin
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
    }
    assert!(!result.unwrap());
    assert!(batch.waiting && batch.has_more);
    let after = *db
        .run_command(doc! {"hello":1})
        .await
        .unwrap()
        .get_datetime("localTime")
        .unwrap();
    let deadline = cursor.drain_until.unwrap().timestamp_millis();
    assert!(
        deadline >= before.timestamp_millis() + expected as i64 * 1000,
        "drain shorter than qualified finality and configured minimum"
    );
    assert!(deadline <= after.timestamp_millis() + expected as i64 * 1000);
    assert!(db
        .collection::<Document>("agent_profiles")
        .find_one(doc! {"owner":seed.owner.as_str(),"account_erasure_fenced":true})
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn readable_lifetime_keeps_the_qualified_finality_bound() {
    deadline(false, 0).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unreadable_lifetime_requires_the_explicit_qualified_bound() {
    deadline(true, 240).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn readable_lifetime_never_shortens_a_larger_configured_floor() {
    deadline(false, 86_400).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn invalid_configured_deadline_requires_cohort_intervention_without_subject_attribution() {
    let seed = seeded("round3_environment_deadline").await;
    let db = seed.client.database(&seed.database);
    let inventory = ErasureInventory::default();
    let result = erase_records_with_options(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut Cursor::default(),
        "2026-11-14T00:00:00Z",
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: u64::MAX,
        },
    )
    .await;
    assert!(
        matches!(
            result,
            Err(Error::CohortBlocked(
                CohortBlock::InvalidTransactionDrain { .. }
            ))
        ),
        "unsupported drain configuration requires operator intervention: {result:?}"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn invalid_worker_clock_is_retryable_without_attributing_the_first_cohort_member() {
    let seed = seeded("round3_invalid_worker_clock").await;
    let db = seed.client.database(&seed.database);
    let inventory = ErasureInventory::default();
    let unrelated = SubjectId::new("unrelated-first-member");
    let targets = [
        Target {
            subject: &unrelated,
            inventory: &inventory,
        },
        Target {
            subject: &seed.owner,
            inventory: &inventory,
        },
    ];
    let mut cursor = Cursor::default();
    let result = erase_records_with_options(
        &db,
        &targets,
        &mut cursor,
        "invalid-worker-clock",
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: 120,
        },
    )
    .await;
    assert!(
        matches!(result, Err(Error::Store(_))),
        "an invalid worker clock must not permanently block a cohort member: {result:?}"
    );
    assert_eq!(
        db.collection::<Document>("agent_profiles")
            .count_documents(doc! {"account_erasure_fenced":true})
            .await
            .unwrap(),
        0
    );
    assert!(
        erase_records(&db, &targets, &mut cursor, "2026-11-14T00:00:00Z")
            .await
            .is_ok()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn server_clock_command_failure_is_retryable_and_a_fresh_attempt_succeeds() {
    let seed = seeded("round3_clock_failure").await;
    let mut settings =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    settings.app_name = Some(seed.database.clone());
    let client = Client::with_options(settings).unwrap();
    let db = client.database(&seed.database);
    db.run_command(doc! {"ping":1}).await.unwrap();
    let admin = seed.client.database("admin");
    admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["hello"],"appName":&seed.database,"errorCode":1}}).await.unwrap();
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &seed.owner,
        inventory: &inventory,
    }];
    let mut cursor = Cursor::default();
    let options = Options {
        allocation_rollups_enabled: Some(false),
        agent_shared_observation: Some(false),
        retain_legal_consents: false,
        drain_qualification: Some(TransactionDrainQualification::fixture()),
        billing_enabled: false,
        transaction_drain_minimum_secs: 120,
    };
    let result =
        super::barrier::ready(&db, &targets, &mut cursor, options, &mut Batch::default()).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(
        matches!(result, Err(Error::Store(_))),
        "a server clock outage must not become an account corruption block: {result:?}"
    );
    assert!(cursor.drain_until.is_none());
    cursor.test_drain_seconds = Some(0);
    assert!(
        super::barrier::ready(&db, &targets, &mut cursor, options, &mut Batch::default())
            .await
            .unwrap()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lifetime_command_failure_is_retryable_without_installing_a_fence() {
    let seed = seeded("drain_lifetime_outage").await;
    let mut settings =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    settings.app_name = Some(seed.database.clone());
    let db = Client::with_options(settings)
        .unwrap()
        .database(&seed.database);
    db.run_command(doc! {"ping":1}).await.unwrap();
    let admin = seed.client.database("admin");
    admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["getParameter"],"appName":&seed.database,"errorCode":1}}).await.unwrap();
    let mut cursor = Cursor::default();
    let result = super::barrier::ready(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &ErasureInventory::default(),
        }],
        &mut cursor,
        Options {
            billing_enabled: false,
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            transaction_drain_minimum_secs: 120,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
        },
        &mut Batch::default(),
    )
    .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(
        matches!(result, Err(Error::Store(_))),
        "outage must remain retryable: {result:?}"
    );
    assert!(cursor.drain_until.is_none());
    assert_eq!(
        db.collection::<Document>("agent_profiles")
            .count_documents(doc! {"account_erasure_fenced":true})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

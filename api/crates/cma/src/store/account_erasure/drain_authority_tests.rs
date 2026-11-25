use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use mongodb::bson::{doc, Document};

const NOW: &str = "2026-11-14T00:00:00Z";

fn options(qualification: Option<TransactionDrainQualification>) -> Options {
    Options {
        billing_enabled: false,
        allocation_rollups_enabled: Some(false),
        agent_shared_observation: Some(false),
        retain_legal_consents: false,
        transaction_drain_minimum_secs: 120,
        drain_qualification: qualification,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unqualified_cleanup_refuses_while_a_real_older_snapshot_remains_open() {
    let seed = seeded("drain_without_authority").await;
    let db = seed.client.database(&seed.database);
    let profiles = db.collection::<Document>("agent_profiles");
    let filter = doc! {"_id":seed.target.agent_profile.id.as_str()};
    let mut session = seed.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let original = profiles
        .find_one(filter.clone())
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let inventory = ErasureInventory::default();
    // A persisted old completion marker cannot replace current qualification.
    let mut cursor = Cursor {
        drain_complete: true,
        ..Default::default()
    };
    let error = erase_records_with_options(
        &db,
        &[Target {
            subject: &seed.owner,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
        options(None),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        Error::CohortBlocked(CohortBlock::TransactionDrainQualificationRequired)
    ));
    assert_eq!(
        profiles.find_one(filter.clone()).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        profiles
            .find_one(filter)
            .session(&mut session)
            .await
            .unwrap(),
        Some(original)
    );
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn changed_qualification_restarts_a_persisted_completed_drain_before_discovery() {
    let seed = seeded("drain_qualification_resume").await;
    let db = seed.client.database(&seed.database);
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &seed.owner,
        inventory: &inventory,
    }];
    let mut cursor = Cursor {
        test_drain_seconds: Some(0),
        ..Default::default()
    };
    let first = erase_records_with_options(
        &db,
        &targets,
        &mut cursor,
        NOW,
        options(Some(TransactionDrainQualification::fixture())),
    )
    .await
    .unwrap();
    assert!(first.deleted.is_empty() && first.redacted.is_empty());
    assert!(cursor.drain_complete);
    let saved = serde_json::to_vec(&cursor).unwrap();
    for changed in [
        TransactionDrainQualification::from_verified_limits([2; 32], 60, 120).unwrap(),
        TransactionDrainQualification::from_verified_limits([1; 32], 60, 240).unwrap(),
    ] {
        let mut resumed: Cursor = serde_json::from_slice(&saved).unwrap();
        let batch =
            erase_records_with_options(&db, &targets, &mut resumed, NOW, options(Some(changed)))
                .await
                .unwrap();
        assert!(batch.waiting && batch.has_more);
        assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
        assert!(batch.discovered.is_empty());
        assert!(!resumed.drain_complete);
        assert!(resumed.contexts.is_empty());
        assert!(resumed.retained_profiles.is_none());
        assert!(resumed.drain_until.is_some());
    }
    db.drop().await.unwrap();
}

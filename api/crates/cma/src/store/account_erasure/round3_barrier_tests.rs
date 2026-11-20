//! Server lifetime cleanup and configured operational drain floors.
use super::{tests::*, *};
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn invalid_worker_clock_is_retryable_without_attributing_the_first_cohort_member() {
    let db = database().await;
    let owner = SubjectId::new("clock-owner");
    let inventory = ErasureInventory::default();
    let unrelated = SubjectId::new("unrelated-first-member");
    let targets = [
        Target {
            subject: &unrelated,
            inventory: &inventory,
        },
        Target {
            subject: &owner,
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
            billing_enabled: false,
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

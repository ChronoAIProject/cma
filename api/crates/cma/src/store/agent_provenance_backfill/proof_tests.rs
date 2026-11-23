use super::*;
use crate::store::agent_admission::mongo_integration_tests::metadata_unknown::fixture::{
    Fixture, Operation,
};

#[derive(Clone, Copy)]
enum Proof {
    Exact,
    MissingTime,
    MissingReceipt,
    Mismatched,
    Corrupt,
    Unreadable,
    ChangedLineage,
}

async fn check(proof: Proof) {
    let f = Fixture::new(Operation::Provenance).await;
    f.run(Operation::Provenance).await.unwrap();
    let mut expected = f.provenance.clone();
    match proof {
        Proof::MissingReceipt => {
            f.independent
                .agent_creation_provenance
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        Proof::Mismatched => expected.agent_created_at = "2026-09-05T00:02:00Z".into(),
        Proof::ChangedLineage => {
            f.independent
                .agent_creation_provenance
                .update_many(
                    doc! {},
                    doc! {"$set":{"record.sandbox_created_at":"2026-09-05T00:02:00Z"}},
                )
                .await
                .unwrap();
        }
        Proof::Corrupt => {
            f.independent
                .agent_creation_provenance
                .update_many(doc! {}, doc! {"$set":{"record":"invalid"}})
                .await
                .unwrap();
        }
        _ => {}
    }
    let at = if matches!(proof, Proof::MissingTime) {
        None
    } else {
        Some(f.snapshot_time().await)
    };
    let before = f.state().await;
    f.reset_observations();
    if matches!(proof, Proof::Unreadable) {
        f.fail("find", 1, None, false).await;
    }
    let result = after_unknown(&f.store, expected, at).await;
    f.clear().await;
    let commands = f.commands();
    let after = f.state().await;
    f.drop_database().await;
    assert!(
        after == before,
        "receipt verification must leave all durable facts intact"
    );
    assert!(commands.iter().all(|(name, command)| !matches!(
        name.as_str(),
        "insert" | "update" | "delete" | "findAndModify" | "commitTransaction"
    ) && command.get_bool("startTransaction").ok()
        != Some(true)));
    if matches!(proof, Proof::Exact) {
        assert_eq!(result.unwrap(), ProvenanceBackfillOutcome::Retained);
    } else {
        assert!(result.is_err(), "inexact proof must refuse without replay");
    }
    if matches!(proof, Proof::MissingTime) {
        assert!(commands.is_empty());
    } else {
        assert!(commands.iter().any(|(name, _)| name == "find"));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn exact_receipt_is_read_only() {
    check(Proof::Exact).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn missing_time_has_no_io() {
    check(Proof::MissingTime).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn missing_receipt_never_replays() {
    check(Proof::MissingReceipt).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn mismatched_receipt_never_replays() {
    check(Proof::Mismatched).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn corrupt_receipt_never_repairs() {
    check(Proof::Corrupt).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unreadable_receipt_never_replays() {
    check(Proof::Unreadable).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn changed_lineage_never_replays() {
    check(Proof::ChangedLineage).await;
}

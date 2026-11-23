use super::*;
use crate::store::agent_admission::mongo_integration_tests::metadata_unknown::fixture::{
    Fixture, Operation, Outcome,
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
    let f = Fixture::new(Operation::Channel).await;
    let Outcome::Channel(expected) = f.run(Operation::Channel).await.unwrap() else {
        panic!("fixture must persist a channel receipt");
    };
    let mut expected = *expected;
    match proof {
        Proof::MissingReceipt => {
            f.independent
                .channel_usage_events
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        Proof::Mismatched => expected.at = "2026-09-05T00:02:00Z".into(),
        Proof::ChangedLineage => {
            expected.after.creation.agent_created_at = "2026-09-05T00:02:00Z".into()
        }
        Proof::Corrupt => {
            f.independent
                .channel_usage_events
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
    let result = verify(&f.store, &f.channel, expected.clone(), at).await;
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
        assert!(result.is_ok_and(|actual| actual == expected));
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

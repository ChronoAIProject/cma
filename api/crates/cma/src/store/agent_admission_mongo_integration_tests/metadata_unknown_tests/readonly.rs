//! Exact receipt proof must never invoke the ordinary reader's quarantine writes.
use super::*;
use crate::store::agent_admission_records::reconcile_existing;

#[derive(Clone, Copy)]
enum Proof {
    Exact,
    MissingTime,
    MissingReceipt,
    Mismatched,
    CorruptSummary,
    MissingLog,
    Unreadable,
}

async fn check(proof: Proof) {
    let f = Fixture::new(Operation::Reuse).await;
    let Outcome::Reuse(expected) = f.run(Operation::Reuse).await.unwrap() else {
        panic!("fixture must create the retained reuse receipt")
    };
    let mut expected = *expected;
    match proof {
        Proof::MissingReceipt => {
            f.independent
                .agent_reuse_bindings
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        Proof::Mismatched => expected.agent.revision += 1,
        Proof::CorruptSummary => {
            f.independent
                .agents
                .update_one(
                    doc! {"_id":expected.agent.id.as_str()},
                    doc! {"$set":{"agent_log_integrity_v1":"invalid-summary"}},
                )
                .await
                .unwrap();
        }
        Proof::MissingLog => {
            f.independent
                .agents
                .update_one(
                    doc! {"_id":expected.agent.id.as_str()},
                    doc! {"$unset":{"agent_log_integrity_v1":""}},
                )
                .await
                .unwrap();
            f.independent
                .agent_log
                .delete_one(doc! {"agent_id":expected.agent.id.as_str(),"seq":1_i64})
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
    let result = reconcile_existing(&f.store, &f.reuse, expected.clone(), at).await;
    f.clear().await;
    let commands = f.commands();
    let after = f.state().await;
    f.drop_database().await;
    assert_eq!(
        after, before,
        "uncertain proof may not quarantine or repair retained data"
    );
    assert!(
        commands.iter().all(|(name, command)| {
            !matches!(
                name.as_str(),
                "insert" | "update" | "delete" | "findAndModify" | "commitTransaction"
            ) && command.get_bool("startTransaction").ok() != Some(true)
        }),
        "receipt proof must be strictly read-only even for corrupt history"
    );
    if matches!(proof, Proof::Exact) {
        assert_eq!(result.unwrap().creation(), &expected);
    } else {
        assert!(
            result.is_err(),
            "missing, mismatched or corrupt proof must refuse"
        );
    }
    if matches!(proof, Proof::MissingTime) {
        assert!(
            commands.is_empty(),
            "missing operation time refuses before storage I/O"
        );
    } else {
        assert!(commands.iter().any(|(name, _)| name == "find"));
    }
}

macro_rules! case {
    ($name:ident, $proof:ident) => {
        #[tokio::test]
        #[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
        async fn $name() { check(Proof::$proof).await; }
    };
}
case!(exact_reuse_proof_is_read_only, Exact);
case!(
    missing_operation_time_refuses_without_storage_io,
    MissingTime
);
case!(missing_reuse_receipt_never_reapplies, MissingReceipt);
case!(mismatched_creation_never_reapplies, Mismatched);
case!(
    corrupt_summary_never_quarantines_during_uncertainty,
    CorruptSummary
);
case!(missing_log_never_quarantines_during_uncertainty, MissingLog);
case!(unreadable_proof_never_reapplies, Unreadable);

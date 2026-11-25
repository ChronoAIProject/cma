//! Preserve uncertainty at the legacy log converter without changing duplicates.
use super::*;
use mongodb::error::{TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT};

#[derive(Clone, Copy)]
enum Fault {
    Unknown { duplicate: bool, transient: bool },
    Duplicate,
    Abort,
}

async fn exercise(fault: Fault) {
    let f = Fixture::new(Operation::Reuse).await;
    let before = f.state().await;
    let (code, labels) = match fault {
        Fault::Unknown {
            duplicate,
            transient,
        } => {
            let mut labels = vec![UNKNOWN_TRANSACTION_COMMIT_RESULT];
            if transient {
                labels.push(TRANSIENT_TRANSACTION_ERROR);
            }
            (if duplicate { 11000 } else { 1 }, labels)
        }
        Fault::Duplicate => (11000, vec![]),
        Fault::Abort => (112, vec![TRANSIENT_TRANSACTION_ERROR]),
    };
    f.fail_insert(code, &labels).await;
    let result = f.run(Operation::Reuse).await;
    f.clear().await;
    let commands = f.commands();
    let observations = f.observations();
    let after = f.state().await;
    f.drop_database().await;

    assert_eq!(
        observations
            .iter()
            .filter(|row| {
                row.get_str("command").ok() == Some("insert")
                    && row.get_i32("error_code").ok() == Some(code)
                    && row.get_bool("unknown").ok()
                        == Some(labels.contains(&UNKNOWN_TRANSACTION_COMMIT_RESULT))
                    && row.get_bool("transient").ok()
                        == Some(labels.contains(&TRANSIENT_TRANSACTION_ERROR))
            })
            .count(),
        1,
        "the requested code and labels must actually reach the driver"
    );
    let retry = matches!(fault, Fault::Abort);
    let transactions = starts(&commands);
    assert_eq!(transactions.len(), if retry { 2 } else { 1 });
    assert_eq!(
        commands
            .iter()
            .filter(|(name, _)| name == "commitTransaction")
            .count(),
        usize::from(retry)
    );
    if retry {
        assert!(
            transactions[0].get("lsid") != transactions[1].get("lsid")
                || transactions[0].get("txnNumber") != transactions[1].get("txnNumber")
        );
        let log_inserts = commands
            .iter()
            .filter(|(_, command)| command.get_str("insert").ok() == Some(f.store.agent_log.name()))
            .map(|(_, command)| command.get_array("documents").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(log_inserts.len(), 2);
        assert!(
            log_inserts[0] == log_inserts[1],
            "retry must retain the original log payload"
        );
        f.assert_committed(Operation::Reuse, result.unwrap(), after);
    } else {
        assert!(after == before, "refusal must preserve all durable state");
        match fault {
            Fault::Duplicate => assert!(matches!(result, Err(SandboxStoreError::SequenceConflict))),
            Fault::Unknown { .. } => {
                assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))))
            }
            Fault::Abort => unreachable!(),
        }
    }
    eprintln!(
        "legacy_reuse_precommit code={code} starts={} commits={} retry={retry}",
        transactions.len(),
        usize::from(retry)
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_only_refuses_without_replay() {
    exercise(Fault::Unknown {
        duplicate: false,
        transient: false,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_transient_refuses_without_replay() {
    exercise(Fault::Unknown {
        duplicate: false,
        transient: true,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_duplicate_refuses_without_sequence_conflict() {
    exercise(Fault::Unknown {
        duplicate: true,
        transient: false,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn duplicate_only_retains_sequence_conflict() {
    exercise(Fault::Duplicate).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn known_abort_retries_original_log_payload() {
    exercise(Fault::Abort).await;
}

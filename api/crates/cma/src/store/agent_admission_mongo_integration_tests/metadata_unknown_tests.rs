//! Unknown commits permit exact receipt recovery, never another mutation attempt.
use super::*;
use layer2_core::resource_access::ResourceActor;
use layer2_core::sandbox::{
    channel_usage::*,
    store::{
        AgentChannelUsageStore, AgentCreation, AgentProvenanceBackfillStore, AgentProvenanceStore,
        ClaimedWork, PageRequest, ProvenanceBackfillOutcome, SandboxWorkerStore, WorkAuthority,
    },
    AgentCreationProvenance, FirstMessageRecord,
};
use tokio::time::Instant;

#[path = "metadata_unknown_tests/fixture.rs"]
pub(in crate::store) mod fixture;
#[path = "metadata_unknown_tests/observations.rs"]
mod observations;
#[path = "metadata_unknown_tests/readonly.rs"]
mod readonly;
#[path = "metadata_unknown_tests/reuse_precommit.rs"]
mod reuse_precommit;
use fixture::{Fixture, Operation, Outcome};

async fn metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_case(
    operation: Operation,
) {
    let f = Fixture::new(operation).await;
    f.fail("commitTransaction", 5, Some(true), false).await;
    let result = f.run(operation).await;
    f.clear_and_settle_commit().await;
    let commands = f.commands();
    let state = f.state().await;
    f.drop_database().await;

    if let Err(error) = &result {
        eprintln!(
                "metadata reconciliation refused: {operation:?} {error:?}; durable collection counts={:?}",
                state.iter().map(Vec::len).collect::<Vec<_>>()
            );
        for (name, command) in &commands {
            if name == "find" {
                eprintln!(
                    "receipt read: collection={:?} concern={:?}",
                    command.get_str("find"),
                    command.get_document("readConcern")
                );
            }
        }
    }
    let observations = f.observations();
    eprintln!(
        "{operation:?}: durable collection counts {:?}; receipt protocol {observations:?}",
        state.iter().map(Vec::len).collect::<Vec<_>>()
    );
    f.assert_durable_receipt(operation, &state);
    assert_receipt_only(&commands, operation);
    observations::assert_committed_replies(&observations);
    f.assert_committed(operation, result.unwrap(), state);
}

async fn metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_case(
    operation: Operation,
) {
    let f = Fixture::new(operation).await;
    let before = f.state().await;
    f.fail("commitTransaction", 5, Some(false), false).await;
    let result = f.run(operation).await;
    f.clear().await;
    let commands = f.commands();
    let after = f.state().await;
    f.drop_database().await;

    assert!(
        matches!(result, Err(SandboxStoreError::Unavailable(_))),
        "{operation:?}: an absent unknown receipt must not authorize reapply: {result:?}"
    );
    assert_eq!(
        after, before,
        "{operation:?}: unknown commit changed durable facts"
    );
    assert_receipt_only(&commands, operation);
}

async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
    operation: Operation,
) {
    let f = Fixture::new(operation).await;
    let claimed = f.claim().await;
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.fail("update", 1, None, false).await;
            crate::store::work_publication::scope(&f.store, f.run(operation)).await
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &fixture::now(), &fixture::deadline())
                .await
        },
    )
    .await;
    f.clear().await;
    let commands = f.commands();
    let state = f.state().await;
    f.drop_database().await;

    let outcome = result.expect("known-aborted startup fence must retry from a fresh snapshot");
    if matches!(operation, Operation::Snapshot | Operation::History) {
        assert!(matches!(outcome, Outcome::Read));
    } else {
        f.assert_committed(operation, outcome, state);
    }
    let starts = starts(&commands);
    assert_eq!(starts.len(), 2, "{operation:?}: exactly one fresh retry");
    assert!(starts
        .iter()
        .all(|command| command.get_str("update").ok() == Some("sandboxes")));
    assert!(
        starts[0].get("lsid") != starts[1].get("lsid")
            || starts[0].get("txnNumber") != starts[1].get("txnNumber")
    );
}

async fn metadata_precommit_unknown_dominates_duplicate_and_transient_labels_case(
    operation: Operation,
) {
    let f = Fixture::new(operation).await;
    let before = f.state().await;
    f.fail("insert", 1, None, true).await;
    let result = f.run(operation).await;
    f.clear().await;
    let commands = f.commands();
    let after = f.state().await;
    f.drop_database().await;

    assert!(
        matches!(result, Err(SandboxStoreError::Unavailable(_))),
        "{operation:?}: {result:?}"
    );
    assert_eq!(after, before);
    assert_eq!(
        starts(&commands).len(),
        1,
        "{operation:?}: uncertain duplicate cannot restart"
    );
    assert!(!commands.iter().any(|(name, _)| name == "commitTransaction"));
    assert_eq!(
        f.observations()
            .iter()
            .filter(|row| {
                row.get_str("command").ok() == Some("insert")
                    && row.get_i32("error_code").ok() == Some(11000)
                    && row.get_bool("unknown").ok() == Some(true)
                    && row.get_bool("transient").ok() == Some(true)
            })
            .count(),
        1,
        "the duplicate-key fault must preserve both uncertainty labels"
    );
}

fn starts(commands: &[(String, Document)]) -> Vec<&Document> {
    commands
        .iter()
        .filter_map(|(_, command)| {
            (command.get_bool("startTransaction").ok() == Some(true)).then_some(command)
        })
        .collect()
}

fn assert_receipt_only(commands: &[(String, Document)], operation: Operation) {
    assert_eq!(
        starts(commands).len(),
        1,
        "{operation:?}: unknown must not start a second transaction"
    );
    let commits = commands
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name == "commitTransaction")
        .collect::<Vec<_>>();
    assert_eq!(
        commits.len(),
        5,
        "{operation:?}: bounded same-transaction commit attempts"
    );
    let original = &commits[0].1 .1;
    assert!(commits.iter().all(|(_, (_, command))| {
        command.get("lsid") == original.get("lsid")
            && command.get("txnNumber") == original.get("txnNumber")
    }));
    let tail = &commands[commits.last().unwrap().0 + 1..];
    assert!(
        tail.iter().any(|(name, _)| name == "find"),
        "{operation:?}: exact receipt was not read"
    );
    assert!(
        tail.iter().all(|(name, command)| {
            !matches!(
                name.as_str(),
                "insert" | "update" | "delete" | "findAndModify" | "commitTransaction"
            ) && command.get_bool("startTransaction").ok() != Some(true)
        }),
        "{operation:?}: reconciliation must only read"
    );
    let concerns = tail
        .iter()
        .filter(|(name, _)| name == "find")
        .map(|(_, command)| command.get_document("readConcern").unwrap())
        .collect::<Vec<_>>();
    let reads = tail
        .iter()
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    let session = reads[0].get("lsid").expect("snapshot session identity");
    assert!(reads.iter().all(|read| read.get("lsid") == Some(session)));
    let at = concerns[0].get_timestamp("atClusterTime").unwrap();
    assert!(concerns.iter().all(|concern| {
        concern.get_str("level").ok() == Some("snapshot")
            && concern.get_timestamp("atClusterTime").ok() == Some(at)
    }));
}

#[path = "metadata_unknown_tests_cases.rs"]
mod cases;

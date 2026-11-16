use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::fixture as seed;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    record, work_tests::live_request,
};
use layer2_core::sandbox::agent_io::{
    AgentCommandWorkerStore, CommandClaimRequest, CommandFailureCode,
};
use std::time::{Duration, SystemTime};
pub(in crate::store) mod fixture;
use fixture::{assert_attempts, Fixture};

#[derive(Clone, Copy)]
enum Root {
    Claim,
    Reject,
}
async fn apply(
    store: &MongoSandboxStore,
    record: &AdmittedAgentCommand,
    root: Root,
) -> Result<(), CommandLedgerError> {
    let now = layer2_core::now_rfc3339();
    match root {
        Root::Claim => store
            .claim_next_command(CommandClaimRequest {
                agent_id: record.receipt.envelope.agent_id.clone(),
                holder: "m2-claimant".into(),
                now,
                expires_at: layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(30)),
            })
            .await
            .map(|value| {
                assert!(value.is_some());
            }),
        Root::Reject => {
            store
                .reject_unclaimed_command(
                    &record.receipt.envelope.command_id,
                    record.revision,
                    CommandFailureCode::AuthorityRevoked,
                    now,
                )
                .await
        }
    }
}
async fn exercise(root: Root, failures: usize, labels: &[&str], stale: bool) {
    let mut f = Fixture::new().await;
    let (parent, agent) = seed(&f.store, "m2-command-fence").await;
    let admitted = record(
        f.store
            .reserve_command(live_request(&agent, &parent, "m2-command-fence"))
            .await
            .unwrap(),
    );
    let authority = f.own(&agent.id).await;
    f.arm(failures, labels, stale);
    let task = tokio::spawn({
        let store = f.store.clone();
        let authority = authority.clone();
        let admitted = admitted.clone();
        async move { authority.run(apply(&store, &admitted, root)).await }
    });
    if stale {
        f.takeover(&authority).await;
    }
    let result = tokio::time::timeout(Duration::from_secs(20), task).await;
    let (hits, commands) = f.clear().await;
    let after = f
        .store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    f.cleanup().await;
    let result = result.expect("must remain bounded").unwrap().unwrap();
    let unknown = labels.contains(&"UnknownTransactionCommitResult");
    if failures == 1 && !unknown && !stale {
        result.expect("known-aborted command ownership fence must retry");
        assert_eq!(after.revision, admitted.revision + 1);
        assert_eq!(after.receipt, admitted.receipt);
        assert_attempts(&commands, 2);
        assert_eq!(hits, 1);
    } else {
        assert!(
            result.is_err(),
            "uncertain or stale attempt must not publish"
        );
        assert_eq!(after, admitted);
        let attempts = if unknown {
            1
        } else if stale {
            2
        } else {
            5
        };
        assert_attempts(&commands, attempts);
        assert_eq!(hits, if stale { 1 } else { attempts });
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_command_claim_known_abort() {
    exercise(Root::Claim, 1, &["TransientTransactionError"], false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_unclaimed_rejection_known_abort() {
    exercise(Root::Reject, 1, &["TransientTransactionError"], false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_commands_exhaustion_unknown_and_both_labels() {
    for root in [Root::Claim, Root::Reject] {
        for labels in [
            vec!["TransientTransactionError"],
            vec!["UnknownTransactionCommitResult"],
            vec![
                "TransientTransactionError",
                "UnknownTransactionCommitResult",
            ],
        ] {
            exercise(root, 5, &labels, false).await;
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_commands_recheck_stale_token() {
    for root in [Root::Claim, Root::Reject] {
        exercise(root, 1, &["TransientTransactionError"], true).await;
    }
}

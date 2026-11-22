use crate::store::sandbox_maintenance::mongo_integration_tests::fixture as seed;
use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    record, work_tests::live_request,
};
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandStore, AgentCommandWorkerStore, CommandClaimRequest,
    CommandFailureCode, CommandLedgerError,
};
use std::time::{Duration, SystemTime};
#[path = "typed_fence_tests/fixture.rs"]
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
) -> Result<bool, CommandLedgerError> {
    let now = layer2_core::now_rfc3339();
    match root {
        Root::Claim => store
            .claim_observed_command(
                CommandClaimRequest {
                    agent_id: record.receipt.envelope.agent_id.clone(),
                    holder: "m2-claimant".into(),
                    now,
                    expires_at: layer2_core::rfc3339_at(
                        SystemTime::now() + Duration::from_secs(30),
                    ),
                },
                &record.receipt.envelope.command_id,
                record.revision,
            )
            .await
            .map(|value| value.is_some()),
        Root::Reject => store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                CommandFailureCode::AuthorityRevoked,
                now,
            )
            .await
            .map(|()| true),
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
    if stale {
        // Claim identity is the current command revision/lease. A runtime owner
        // token does not authorize overwriting a command acquired by a successor.
        let successor = f
            .store
            .claim_observed_command(
                CommandClaimRequest {
                    agent_id: agent.id.clone(),
                    holder: "successor".into(),
                    now: layer2_core::now_rfc3339(),
                    expires_at: layer2_core::rfc3339_at(
                        SystemTime::now() + Duration::from_secs(30),
                    ),
                },
                &admitted.receipt.envelope.command_id,
                admitted.revision,
            )
            .await
            .unwrap()
            .unwrap();
        let result = authority
            .run(apply(&f.store, &admitted, root))
            .await
            .unwrap();
        let after = f
            .store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        f.cleanup().await;
        match root {
            Root::Claim => assert_eq!(result, Ok(false)),
            Root::Reject => assert_eq!(result, Err(CommandLedgerError::StaleClaim)),
        }
        assert_eq!(after, successor);
        assert_eq!(after.receipt, admitted.receipt);
        return;
    }
    f.arm_commands(failures, labels);
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        authority.run(apply(&f.store, &admitted, root)),
    )
    .await;
    let (hits, commands) = f.clear().await;
    let after = f
        .store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let unknown = labels.contains(&"UnknownTransactionCommitResult");
    let explicit_retry = if matches!(root, Root::Reject) && failures == 1 && !unknown {
        Some(apply(&f.store, &admitted, root).await)
    } else {
        None
    };
    let final_record = f
        .store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    f.cleanup().await;
    let result = result.expect("must remain bounded").unwrap();
    if matches!(root, Root::Reject) {
        // This public rejection is a single revision CAS, not a transaction.
        // An error leaves the original receipt intact; only an explicit later
        // request may repeat the unchanged original revision after known abort.
        assert_eq!(result, Err(CommandLedgerError::Unavailable));
        assert_eq!(after, admitted);
        assert_attempts(&commands, 0);
        assert_eq!(hits, 1);
        if let Some(retry) = explicit_retry {
            assert_eq!(retry, Ok(true));
            assert_eq!(final_record.revision, admitted.revision + 1);
            assert_eq!(final_record.receipt, admitted.receipt);
        } else {
            assert_eq!(final_record, admitted);
        }
    } else if failures == 1 && !unknown {
        assert_eq!(result, Ok(true));
        assert_eq!(after.revision, admitted.revision + 1);
        assert_eq!(after.receipt, admitted.receipt);
        assert_attempts(&commands, 2);
        assert_eq!(hits, 1);
    } else {
        assert!(
            result.is_err(),
            "uncertain or exhausted claim must not publish"
        );
        assert_eq!(after, admitted);
        assert_eq!(final_record, admitted);
        let attempts = if unknown { 1 } else { 5 };
        assert_attempts(&commands, attempts);
        assert_eq!(hits, attempts);
    }
    assert!(!commands
        .iter()
        .any(|command| command.get_str("update").ok() == Some("cma_agent_manager_members")));
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
async fn typed_fence_commands_exhaustion_unknown_and_both_labels_case(
    root: Root,
    labels: Vec<&'static str>,
) {
    exercise(root, 5, &labels, false).await;
}
async fn typed_fence_commands_recheck_stale_token_case(root: Root) {
    exercise(root, 1, &["TransientTransactionError"], true).await;
}

#[path = "typed_fence_tests_cases.rs"]
mod cases;

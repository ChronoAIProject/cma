//! Real transaction roots must retain known-aborted WorkAuthority driver errors.
use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::{
    sandbox_lifecycle_tests::{checkpoint_for_agents, checkpoint_mutation, claimed_pause_fixture},
    work_lease_deadline,
};
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, ClaimedWork, SandboxEventStore, SandboxLifecycleAgentMutation,
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxStore, SandboxStoreError, SandboxWorkerStore, WorkAuthority,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, Sandbox, SandboxEventKind, SandboxLifecycleOperation,
    SandboxPausePhase, SandboxWorkStep,
};
use mongodb::{
    bson::{doc, Document},
    Client,
};
use std::time::Duration;
use tokio::time::Instant;
#[path = "authority_mongo_tests/fixture.rs"]
mod fixture;
#[path = "authority_mongo_tests/ledger_maintenance.rs"]
mod ledger_maintenance;
#[path = "authority_mongo_tests/purge.rs"]
mod purge;
#[path = "authority_mongo_tests/usage.rs"]
mod usage;
#[path = "authority_mongo_tests/workspace.rs"]
mod workspace;
use fixture::{now, Fixture};
const MINUTE: &str = "2026-09-05T00:01:00Z";

async fn publish(
    f: &Fixture,
    claimed: &Sandbox,
    mutation: SandboxLifecycleMutation,
    scoped: bool,
    failures: i32,
    unknown_commit: bool,
) -> Result<(), SandboxStoreError> {
    WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.fail(
                if unknown_commit {
                    "commitTransaction"
                } else {
                    "update"
                },
                failures,
                unknown_commit,
            )
            .await;
            let work = f.store.commit_sandbox_lifecycle(mutation);
            if scoped {
                crate::store::work_publication::scope(&f.store, work).await
            } else {
                work.await
            }
        },
        || async {
            f.independent
                .renew_sandbox_authority(claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await
}

fn assert_fresh_transactions(commands: &[Document], count: usize) {
    assert_eq!(
        commands.len(),
        count,
        "each known abort needs a new transaction"
    );
    for (index, command) in commands.iter().enumerate() {
        assert_eq!(
            command
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "snapshot"
        );
        for earlier in &commands[..index] {
            assert!(
                command.get_document("lsid").unwrap() != earlier.get_document("lsid").unwrap()
                    || command.get_i64("txnNumber").unwrap()
                        != earlier.get_i64("txnNumber").unwrap(),
                "an aborted snapshot cannot be reused"
            );
        }
    }
}

async fn known_abort_at_work_authority_fence_retries_fresh_and_publishes_children_once_case(
    scoped: bool,
) {
    let f = Fixture::new().await;
    let (claimed, agents, mutation) = f.phase().await;
    let result = publish(&f, &claimed, mutation.clone(), scoped, 1, false).await;
    f.clear().await;
    let starts = f.starts();
    let observed = f.observations(&claimed, &agents).await;
    f.drop_database().await;
    result.expect("known-aborted WorkAuthority fence must retry the lifecycle transaction");
    assert_fresh_transactions(&starts, 2);
    assert_eq!(observed.0, Some(mutation.sandbox));
    assert_eq!(
        observed.1,
        mutation
            .agents
            .into_iter()
            .map(|change| change.agent)
            .collect::<Vec<_>>()
    );
    assert_eq!(observed.2, vec![1, 1]);
    assert_eq!(observed.3, 5);
}

async fn repeated_work_authority_abort_is_bounded_and_leaves_no_partial_publication_case(
    scoped: bool,
) {
    let f = Fixture::new().await;
    let (claimed, agents, mutation) = f.phase().await;
    let before = f.observations(&claimed, &agents).await;
    let start = Instant::now();
    let result = publish(&f, &claimed, mutation, scoped, 5, false).await;
    let elapsed = start.elapsed();
    f.clear().await;
    let starts = f.starts();
    let after = f.observations(&claimed, &agents).await;
    f.drop_database().await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_fresh_transactions(&starts, 5);
    assert!(elapsed < Duration::from_secs(10));
    // Renewal may update the lease expiry, but no phase or child publication advances.
    assert_eq!(
        after.0.as_ref().unwrap().revision,
        before.0.as_ref().unwrap().revision
    );
    assert_eq!(
        after.0.as_ref().unwrap().lifecycle,
        before.0.as_ref().unwrap().lifecycle
    );
    assert_eq!((after.1, after.2, after.3), (before.1, before.2, before.3));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn work_authority_unknown_commit_reconciles_exact_phase_without_replaying_it() {
    let f = Fixture::new().await;
    let (claimed, agents, mutation) = f.phase().await;
    let result = publish(&f, &claimed, mutation.clone(), true, 5, true).await;
    f.clear().await;
    let starts = f.starts();
    let observed = f.observations(&claimed, &agents).await;
    f.drop_database().await;
    result.expect("unknown commit must use the existing exact receipt reconciliation");
    // Receipt reads may open later snapshot transactions; only one writes the claim.
    assert_eq!(
        starts
            .iter()
            .filter(|command| command.get_str("update").ok() == Some("sandboxes"))
            .count(),
        1
    );
    assert_eq!(observed.0, Some(mutation.sandbox));
    assert_eq!(observed.2, vec![1, 1]);
    assert_eq!(observed.3, 5);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn superseded_work_authority_is_refused_without_retrying_or_publishing_children() {
    let f = Fixture::new().await;
    let (claimed, agents, mutation) = f.phase().await;
    let before = f.observations(&claimed, &agents).await;
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.independent
                .sandboxes
                .update_one(
                    doc! {"_id":claimed.id.as_str()},
                    doc! {
                        "$set":{"record.work.lease.expires_at":MINUTE,"work_lease_expires_at":MINUTE},
                    },
                )
                .await
                .unwrap();
            // A competing worker has its own task-local authority. Reusing this
            // task would correctly constrain its claim to the old worker fence.
            let successor = f.independent.clone();
            let id = claimed.id.clone();
            tokio::spawn(async move {
                successor
                    .claim_sandbox_work(&id, "successor", MINUTE.into(), work_lease_deadline())
                    .await
            })
            .await
            .expect("successor worker must join")
            .expect("successor claim must not inherit the old authority")
            .expect("expired claim must be taken over by another worker");
            f.store.commit_sandbox_lifecycle(mutation).await
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await;
    let observed = f.observations(&claimed, &agents).await;
    f.drop_database().await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        observed
            .0
            .as_ref()
            .unwrap()
            .work
            .lease
            .as_ref()
            .unwrap()
            .holder,
        "successor"
    );
    assert_eq!(
        (observed.1, observed.2, observed.3),
        (before.1, before.2, before.3)
    );
}

async fn checkpoint_publication_retries_only_aborted_authority_fence_case(scoped: bool) {
    let f = Fixture::new().await;
    let (claimed, agents) = claimed_pause_fixture(&f.independent, "checkpoint_fence").await;
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for_agents(&agents, 1));
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            f.fail("update", 1, false).await;
            let work = f.store.publish_sandbox_checkpoint(mutation.clone());
            if scoped {
                crate::store::work_publication::scope(&f.store, work).await
            } else {
                work.await
            }
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await;
    f.clear().await;
    let starts = f.starts();
    let observed = f.observations(&claimed, &agents).await;
    let checkpoints = f
        .independent
        .checkpoints
        .count_documents(doc! {"sandbox_id":claimed.id.as_str()})
        .await
        .unwrap();
    f.drop_database().await;
    result.expect("checkpoint authority fence must retain known-aborted retry evidence");
    assert_fresh_transactions(&starts, 2);
    assert_eq!(observed.0, Some(mutation.sandbox));
    assert_eq!(observed.1, agents);
    assert_eq!(observed.2, vec![0, 0]);
    assert_eq!(observed.3, 2);
    assert_eq!(checkpoints, 1);
}

#[path = "authority_mongo_tests_cases.rs"]
mod cases;

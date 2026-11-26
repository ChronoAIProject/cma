//! Real owner-scoped Agent writes must retain known-aborted fence errors.
use super::*;
use crate::store::sandbox_mongo::{MongoSandboxStore, TRANSACTION_ATTEMPTS};
use layer2_core::agent_ownership::Authority;
use layer2_core::sandbox::store::AgentStore;
use layer2_core::sandbox::Agent;
use std::sync::Arc;
use std::time::Duration;

#[path = "fence_mongo_tests/fixture.rs"]
mod fixture;
use fixture::Fixture;

fn mutation(agent: &Agent) -> Agent {
    let mut next = agent.clone();
    next.revision += 1;
    next.updated_at = layer2_core::now_rfc3339();
    next
}

fn starts(commands: &[Document]) -> Vec<&Document> {
    commands
        .iter()
        .filter(|command| matches!(command.get_bool("startTransaction"), Ok(true)))
        .collect()
}

fn assert_fresh(commands: &[Document], count: usize) {
    let started = starts(commands);
    assert_eq!(started.len(), count);
    for (index, command) in started.iter().enumerate() {
        for earlier in &started[..index] {
            assert!(
                command.get_document("lsid").unwrap() != earlier.get_document("lsid").unwrap()
                    || command.get_i64("txnNumber").unwrap()
                        != earlier.get_i64("txnNumber").unwrap(),
                "a known-aborted ownership snapshot must not be reused"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn ownership_fence_known_abort_retries_actual_agent_write_without_changing_token() {
    let f = Fixture::new().await;
    let before = f.snapshot().await;
    let next = mutation(&f.agent);
    f.fault("update", 1, false, false).await;
    let result = f
        .authority
        .run(f.store.save_agent(f.agent.revision, next.clone()))
        .await;
    let commands = f.clear().await;
    let after = f.snapshot().await;
    let owner = f.independent.owner(&f.agent.id).await.unwrap().unwrap();
    f.cleanup().await;
    result
        .unwrap()
        .expect("known-aborted owner fence must retry the Agent transaction");
    assert_fresh(&commands, 2);
    assert_eq!(after.0, next);
    assert_eq!(
        after.1,
        before.1 + 1,
        "aborted member fence cannot accumulate"
    );
    assert_eq!(after.2, before.2 + 1, "only one exact owner fence commits");
    assert_eq!(owner.token, f.authority.token);
}

async fn ownership_fence_exhaustion_and_unknown_commit_never_publish_or_rebase_case(unknown: bool) {
    let f = Fixture::new().await;
    let before = f.snapshot().await;
    f.fault(
        if unknown {
            "commitTransaction"
        } else {
            "update"
        },
        0,
        unknown,
        false,
    )
    .await;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        f.authority
            .run(f.store.save_agent(f.agent.revision, mutation(&f.agent))),
    )
    .await;
    let commands = f.clear().await;
    let after = f.snapshot().await;
    f.cleanup().await;
    let result = result
        .expect("owner fence retry must remain bounded")
        .unwrap();
    if unknown {
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    } else {
        // Preserve this family's existing known-abort exhaustion contract.
        assert_eq!(result, Err(SandboxStoreError::Conflict));
    }
    assert_fresh(&commands, if unknown { 1 } else { TRANSACTION_ATTEMPTS });
    assert_eq!(after, before, "no partial Agent or ownership publication");
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn ownership_fence_rechecks_member_and_exact_epoch_after_known_abort() {
    let f = Fixture::new().await;
    let next = mutation(&f.agent);
    let count = f.fault("update", 1, false, true).await;
    let pending = tokio::spawn({
        let store = f.store.clone();
        let authority = f.authority.clone();
        let revision = f.agent.revision;
        async move { authority.run(store.save_agent(revision, next)).await }
    });
    f.wait_for_fault(count).await;
    f.independent.heartbeat("original", true).await.unwrap();
    f.independent.release(&f.authority.token).await.unwrap();
    f.independent
        .register(fixture::member("successor"))
        .await
        .unwrap();
    let successor = f
        .independent
        .acquire(&f.agent.id, "successor")
        .await
        .unwrap()
        .unwrap();
    let result = pending.await.unwrap();
    let commands = f.clear().await;
    let after = f.snapshot().await;
    let owner = f.independent.owner(&f.agent.id).await.unwrap().unwrap();
    f.cleanup().await;
    assert_eq!(result.unwrap(), Err(SandboxStoreError::Conflict));
    assert_fresh(&commands, 2);
    assert_eq!(
        after.0, f.agent,
        "old token cannot publish into the new epoch"
    );
    assert!(successor.token.epoch > f.authority.token.epoch);
    assert_eq!(owner.token, successor.token);
}

#[path = "fence_mongo_cancellation_tests.rs"]
mod cancellation;
#[path = "fence_mongo_tests_cases.rs"]
mod cases;

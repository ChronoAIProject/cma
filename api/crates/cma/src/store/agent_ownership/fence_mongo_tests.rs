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

#[path = "fence_mongo_tests_cases.rs"]
mod cases;

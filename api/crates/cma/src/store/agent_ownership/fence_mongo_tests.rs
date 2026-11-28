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

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn connection_recovery_survives_same_member_renewal_contention() {
    use layer2_core::sandbox::store::{
        AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, AgentWorkerStore,
    };
    use layer2_core::sandbox::AgentWorkStep;

    let f = Fixture::new().await;
    let renewing = f
        .independent
        .acquire(&AgentId("another-renewing-agent".into()), "original")
        .await
        .unwrap()
        .unwrap();
    let request = AgentConnectionRecoveryCandidate::from_agent(&f.agent)
        .unwrap()
        .restart_request(layer2_core::now_rfc3339());
    let before = f.snapshot().await;

    // Hold the exact shared member row as a renewal would. The Agent owner
    // rows differ, so this isolates the member-fence contention.
    let mut held = f.independent.client.start_session().await.unwrap();
    start_transaction(&mut held).await.unwrap();
    let write = f
        .independent
        .members
        .update_one(
            doc! {"_id": "original"},
            doc! {"$inc": {"renewal_revision": 1_i64}},
        )
        .session(&mut held)
        .await
        .unwrap();
    assert_eq!(write.matched_count, 1);
    let (scheduled, committed) = tokio::join!(
        f.authority
            .run(f.store.schedule_agent_connection_recovery(request.clone())),
        async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            commit_transaction(&mut held).await
        },
    );
    // A real renewal must still make progress on the same member and token.
    let renewed = f.independent.renew(&renewing.token).await;
    let repeated = f
        .authority
        .run(f.store.schedule_agent_connection_recovery(request))
        .await;
    let after = f.snapshot().await;
    let owner = f.independent.owner(&f.agent.id).await.unwrap().unwrap();
    let commands = f.clear().await;
    let member_conflicts = f.member_write_conflicts();
    f.cleanup().await;

    assert!(matches!(committed, CommitOutcome::Committed));
    assert_eq!(
        scheduled.unwrap().unwrap(),
        AgentConnectionRecoveryOutcome::Scheduled
    );
    assert!(
        member_conflicts > 0,
        "the scheduling member write must lose a real code-112 conflict"
    );
    assert_eq!(
        repeated.unwrap().unwrap(),
        AgentConnectionRecoveryOutcome::AlreadyScheduled
    );
    assert_eq!(after.0.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(after.0.revision, before.0.revision + 1);
    assert_eq!(after.0.conversation, before.0.conversation);
    assert_eq!((after.1, after.2), (before.1 + 1, before.2 + 1));
    assert_eq!(owner.token, f.authority.token);
    assert_eq!(renewed.unwrap().token, renewing.token);
    let attempts = starts(&commands).len();
    assert!((2..=TRANSACTION_ATTEMPTS).contains(&attempts));
    assert_fresh(&commands, attempts);
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn connection_recovery_reports_confirmed_contention_exhaustion() {
    use layer2_core::sandbox::store::{AgentConnectionRecoveryCandidate, AgentWorkerStore};

    let f = Fixture::new().await;
    let before = f.snapshot().await;
    let request = AgentConnectionRecoveryCandidate::from_agent(&f.agent)
        .unwrap()
        .restart_request(layer2_core::now_rfc3339());
    f.fault("update", 0, false, false).await;
    let result = f
        .authority
        .run(f.store.schedule_agent_connection_recovery(request))
        .await;
    let commands = f.clear().await;
    let member_conflicts = f.member_write_conflicts();
    let after = f.snapshot().await;
    f.cleanup().await;

    assert_eq!(
        result.unwrap(),
        Err(SandboxStoreError::Unavailable(
            "connection recovery scheduling transaction retry budget exhausted".into()
        ))
    );
    assert_eq!(member_conflicts, TRANSACTION_ATTEMPTS);
    assert_fresh(&commands, TRANSACTION_ATTEMPTS);
    assert_eq!(after, before);
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn connection_recovery_unknown_commit_does_not_start_new_transaction() {
    use layer2_core::sandbox::store::{AgentConnectionRecoveryCandidate, AgentWorkerStore};

    let f = Fixture::new().await;
    let before = f.snapshot().await;
    let request = AgentConnectionRecoveryCandidate::from_agent(&f.agent)
        .unwrap()
        .restart_request(layer2_core::now_rfc3339());
    f.fault("commitTransaction", 0, true, false).await;
    let result = f
        .authority
        .run(f.store.schedule_agent_connection_recovery(request))
        .await;
    let commands = f.clear().await;
    let after = f.snapshot().await;
    f.cleanup().await;

    assert!(
        matches!(result.unwrap(), Err(SandboxStoreError::Unavailable(detail))
        if !detail.contains("retry budget exhausted"))
    );
    assert_fresh(&commands, 1);
    assert_eq!(after, before);
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn connection_recovery_rechecks_owner_after_confirmed_abort() {
    use layer2_core::sandbox::store::{
        AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, AgentWorkerStore,
    };

    let f = Fixture::new().await;
    let request = AgentConnectionRecoveryCandidate::from_agent(&f.agent)
        .unwrap()
        .restart_request(layer2_core::now_rfc3339());
    let count = f.fault("update", 1, false, true).await;
    let pending = tokio::spawn({
        let store = f.store.clone();
        let authority = f.authority.clone();
        async move {
            authority
                .run(store.schedule_agent_connection_recovery(request))
                .await
        }
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

    assert_eq!(
        result.unwrap().unwrap(),
        AgentConnectionRecoveryOutcome::Superseded
    );
    assert_fresh(&commands, 2);
    assert_eq!(after.0, f.agent);
    assert_eq!(owner.token, successor.token);
    assert!(successor.token.epoch > f.authority.token.epoch);
}

use super::{support::*, *};
use crate::store::{
    agent_capacity_recovery::settlement, agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope,
};

fn write_conflict(result: Result<RecoveryWork, TransactionLogError>) {
    assert!(
        matches!(result, Err(TransactionLogError::Driver(ref error))
        if error.contains_label("TransientTransactionError")),
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_invalidates_old_completion_snapshot_and_retry_recounts() {
    let (client, name) = database("quota_stale_completion").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let at = layer2_core::now_rfc3339();
    let claim = claim(&store, &caller, &request, &at).await;
    let authorized = transactions::authority(&store, &claim.agent).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scope
        .find_one(&store.agents, doc! {"_id":request.agent_id.as_str()})
        .await
        .unwrap();
    ordinary(&store, &request, "racing-admission").await;
    let before = durable(&store).await;
    let result = settlement::complete(
        &store,
        &authorized,
        &claim,
        &confirmation(&claim),
        &at,
        &mut scope,
    )
    .await;
    let _ = scope.release().abort_transaction().await;
    write_conflict(result);
    assert_eq!(durable(&store).await, before);
    assert_eq!(
        complete(&store, &caller, &claim, &at).await,
        Err(RecoveryError::WorkspaceCapacityFull { limit: 1, live: 1 })
    );
    assert_eq!(durable(&store).await, before);
    assert_eq!(live(&store, &request).await, 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_recoveries_cannot_publish_into_the_last_slot() {
    let (client, name) = database("quota_two_recoveries").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let (other_caller, other_request, _) = second_recovery(&store, &request).await;
    let at = layer2_core::now_rfc3339();
    let first = claim(&store, &caller, &request, &at).await;
    let second = claim(&store, &other_caller, &other_request, &at).await;
    let authorized = transactions::authority(&store, &second.agent).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scope
        .find_one(&store.agents, doc! {"_id":second.agent.id.as_str()})
        .await
        .unwrap();
    assert!(matches!(
        complete(&store, &caller, &first, &at).await.unwrap(),
        CommitOutcome::Recorded(_)
    ));
    let before = durable(&store).await;
    let result = settlement::complete(
        &store,
        &authorized,
        &second,
        &confirmation(&second),
        &at,
        &mut scope,
    )
    .await;
    let _ = scope.release().abort_transaction().await;
    write_conflict(result);
    assert_eq!(
        complete(&store, &other_caller, &second, &at).await,
        Err(RecoveryError::WorkspaceCapacityFull { limit: 1, live: 1 })
    );
    assert_eq!(durable(&store).await, before);
    assert_eq!(live(&store, &request).await, 1);
    assert_eq!(
        store
            .get_agent(&other_request.agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Failed
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn aborted_completion_does_not_reserve_capacity_for_a_fresh_attempt() {
    let (client, name) = database("quota_aborted_completion").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let at = layer2_core::now_rfc3339();
    let claim = claim(&store, &caller, &request, &at).await;
    let before = durable(&store).await;
    let authorized = transactions::authority(&store, &claim.agent).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let proposed = settlement::complete(
        &store,
        &authorized,
        &claim,
        &confirmation(&claim),
        &at,
        &mut scope,
    )
    .await
    .unwrap();
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(durable(&store).await, before);
    ordinary(&store, &request, "after-abort").await;
    let before = durable(&store).await;
    assert_eq!(
        complete(&store, &caller, &claim, &at).await,
        Err(RecoveryError::WorkspaceCapacityFull { limit: 1, live: 1 })
    );
    assert_eq!(
        service(&store, false)
            .reconcile(&caller, &proposed)
            .await
            .unwrap(),
        CommitOutcome::Unknown
    );
    assert_eq!(durable(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

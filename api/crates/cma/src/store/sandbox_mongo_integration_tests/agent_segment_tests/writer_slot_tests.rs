use super::*;
use mongodb::bson::DateTime;

async fn open_gate(client: &Client, db: &mongodb::Database, agent: &Agent) {
    let mut session = transaction(client).await;
    gate::replace(
        db,
        gate::GateName::Segments,
        0,
        selected(agent),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
}

async fn accepted(
    client: &Client,
    db: &mongodb::Database,
    agent: &Agent,
    key: &str,
) -> (PublicationIdentity, admission::Admission) {
    let mut request = operation();
    request.key = key.into();
    let mut session = transaction(client).await;
    let admitted = admission::admit(
        db,
        agent,
        &request,
        admission::AdmissionKind::Activation,
        NOW,
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    (request, admitted)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn preparation_slots_bound_cross_replica_work_and_fence_reclaimed_holders() {
    let (client, name, _, agent) = fixture("segment_slot_bound").await;
    let db = client.database(&name);
    open_gate(&client, &db, &agent).await;
    let first = accepted(&client, &db, &agent, "fixture-first").await;
    let second = accepted(&client, &db, &agent, "fixture-second").await;
    let third = accepted(&client, &db, &agent, "fixture-third").await;
    let mut session = transaction(&client).await;
    let original = gate::slots::claim(
        &db,
        &agent,
        &first.0,
        &first.1,
        DateTime::now(),
        &mut session,
    )
    .await
    .unwrap()
    .unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    gate::slots::claim(
        &db,
        &agent,
        &second.0,
        &second.1,
        DateTime::now(),
        &mut session,
    )
    .await
    .unwrap()
    .unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    assert!(matches!(
        gate::slots::claim(
            &db,
            &agent,
            &third.0,
            &third.1,
            DateTime::now(),
            &mut session
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    let renewed = gate::slots::claim(
        &db,
        &agent,
        &first.0,
        &first.1,
        DateTime::now(),
        &mut session,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(renewed.fence() > original.fence());
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    assert!(original.consume(&db, &first.1, &mut session).await.is_err());
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    gate::replace(
        &db,
        gate::GateName::Segments,
        1,
        gate::Policy::default(),
        &mut session,
    )
    .await
    .unwrap();
    commit(&mut session).await;
    // The exact accepted operation can publish after closure, with its current slot.
    let mut session = transaction(&client).await;
    renewed.consume(&db, &first.1, &mut session).await.unwrap();
    // A known abort restores the slot with all publication effects.
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    renewed.consume(&db, &first.1, &mut session).await.unwrap();
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    assert!(renewed.consume(&db, &first.1, &mut session).await.is_err());
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn expired_preparation_and_timestamp_drift_never_authorize_publication() {
    let (client, name, _, agent) = fixture("segment_slot_expiry").await;
    let db = client.database(&name);
    open_gate(&client, &db, &agent).await;
    let (request, admitted) = accepted(&client, &db, &agent, "fixture-expiry").await;
    let mut session = transaction(&client).await;
    let permit = gate::slots::claim(
        &db,
        &agent,
        &request,
        &admitted,
        DateTime::from_millis(DateTime::now().timestamp_millis() - 118_000),
        &mut session,
    )
    .await
    .unwrap()
    .unwrap();
    commit(&mut session).await;
    let mut changed = request.clone();
    changed.appended_at = "2026-11-12T00:00:00Z".into();
    let mut session = transaction(&client).await;
    assert!(admission::read(&db, &agent, &changed, &mut session)
        .await
        .is_err());
    session.abort_transaction().await.unwrap();
    // Let the real server-time predicate expire this short test lease.
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let mut session = transaction(&client).await;
    assert!(permit.consume(&db, &admitted, &mut session).await.is_err());
    session.abort_transaction().await.unwrap();
    let mut session = transaction(&client).await;
    let replacement = gate::slots::claim(
        &db,
        &agent,
        &request,
        &admitted,
        DateTime::now(),
        &mut session,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(replacement.fence() > permit.fence());
    commit(&mut session).await;
    let mut session = transaction(&client).await;
    replacement
        .consume(&db, &admitted, &mut session)
        .await
        .unwrap();
    commit(&mut session).await;
    db.drop().await.unwrap();
}

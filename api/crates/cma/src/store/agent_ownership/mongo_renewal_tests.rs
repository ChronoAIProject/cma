//! A different Agent's bounded effect must not exhaust a live owner's renewal.
use super::*;
use layer2_core::agent_ownership::bounded;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn renewal_survives_bounded_contention_from_another_agents_effect() {
    let store = renewal_fixture().await;
    store.register(member("renewal-member")).await.unwrap();
    let effect = store
        .acquire(&AgentId("effect-agent".into()), "renewal-member")
        .await
        .unwrap()
        .unwrap();
    let renewing = store
        .acquire(&AgentId("renewing-agent".into()), "renewal-member")
        .await
        .unwrap()
        .unwrap();

    // The owner rows differ. Only the member row is shared by the effect's
    // fencing transaction and the other Agent's renewal transaction.
    let mut effect_session = store.client.start_session().await.unwrap();
    start_transaction(&mut effect_session).await.unwrap();
    store
        .fence_transaction(&effect.token, &mut effect_session)
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let (renewed, committed) = tokio::join!(bounded(store.renew(&renewing.token)), async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        commit_transaction(&mut effect_session).await
    });
    let membership = store.membership().await.unwrap();
    let member_still_live = membership.members.iter().any(|member| {
        member.instance_id == "renewal-member"
            && !member.draining
            && member.protocol == PROTOCOL_VERSION
            && member.expires_at_ms > membership.now_ms
    });
    let original_owner = store.owner(&renewing.token.agent_id).await.unwrap();
    store.database.drop().await.unwrap();

    eprintln!(
        "bounded_member_contention elapsed_ms={} renewal={renewed:?}",
        started.elapsed().as_millis()
    );
    assert!(matches!(committed, CommitOutcome::Committed));
    assert!(member_still_live, "the effect did not expire membership");
    assert_eq!(
        original_owner.as_ref().map(|lease| &lease.token),
        Some(&renewing.token)
    );
    assert!(
        renewed.is_ok(),
        "a bounded effect on another Agent must not lose a live owner's renewal: {renewed:?}"
    );
    assert_eq!(renewed.unwrap().token, renewing.token);
}

// The reserved native runner supplies a single scoped database. Hosted CI
// retains its existing independently disposable database per test.
async fn renewal_fixture() -> MongoAgentOwnership {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("use the disposable Mongo integration harness");
    let database = match std::env::var("CHRONO_TEST_MONGODB_DATABASE") {
        Ok(database) => database,
        Err(std::env::VarError::NotPresent) => return fixture().await,
        Err(error) => panic!("invalid disposable Mongo database: {error}"),
    };
    assert!(
        database.starts_with("cma_"),
        "use a disposable CMA database"
    );
    let client = Client::with_uri_str(uri).await.unwrap();
    MongoAgentOwnership::open(&client, &database).await.unwrap()
}
#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn renewal_rechecks_expiry_drain_and_epoch_after_contention() {
    for change in ["expired", "draining", "new_epoch"] {
        let store = renewal_fixture().await;
        store.register(member("renewal-member")).await.unwrap();
        let effect = store
            .acquire(&AgentId("effect-agent".into()), "renewal-member")
            .await
            .unwrap()
            .unwrap();
        let renewing = store
            .acquire(&AgentId("renewing-agent".into()), "renewal-member")
            .await
            .unwrap()
            .unwrap();
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .fence_transaction(&effect.token, &mut session)
            .await
            .unwrap();
        match change {
            "expired" => {
                store
                    .members
                    .update_one(
                        doc! {"_id": "renewal-member"},
                        doc! {"$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)}},
                    )
                    .session(&mut session)
                    .await
                    .unwrap();
            }
            "draining" => {
                store
                    .members
                    .update_one(
                        doc! {"_id": "renewal-member"},
                        doc! {"$set": {"draining": true}},
                    )
                    .session(&mut session)
                    .await
                    .unwrap();
            }
            _ => {
                store
                    .owners
                    .update_one(
                        doc! {"_id": renewing.token.agent_id.as_str()},
                        doc! {"$inc": {"epoch": 1_i64}},
                    )
                    .session(&mut session)
                    .await
                    .unwrap();
            }
        }
        let (renewed, committed) = tokio::join!(bounded(store.renew(&renewing.token)), async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            commit_transaction(&mut session).await
        });
        let stale = store.check(&renewing.token).await;
        store.database.drop().await.unwrap();
        assert!(matches!(committed, CommitOutcome::Committed));
        assert_eq!(renewed, Err(OwnershipError::Stale), "{change}");
        assert_eq!(stale, Err(OwnershipError::Stale), "{change}");
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn sustained_member_contention_still_exhausts_the_store_deadline() {
    let store = renewal_fixture().await;
    store.register(member("renewal-member")).await.unwrap();
    let effect = store
        .acquire(&AgentId("effect-agent".into()), "renewal-member")
        .await
        .unwrap()
        .unwrap();
    let renewing = store
        .acquire(&AgentId("renewing-agent".into()), "renewal-member")
        .await
        .unwrap()
        .unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .fence_transaction(&effect.token, &mut session)
        .await
        .unwrap();
    let ((renewed, elapsed), aborted) = tokio::join!(
        async {
            let started = tokio::time::Instant::now();
            // Exercise the store's own total budget, without an outer test timeout
            // converting an accidental unbounded retry into the expected result.
            let renewed =
                tokio::time::timeout(Duration::from_secs(3), store.renew(&renewing.token))
                    .await
                    .expect("store did not enforce its own deadline");
            (renewed, started.elapsed())
        },
        async {
            tokio::time::sleep(Duration::from_millis(2_400)).await;
            session.abort_transaction().await
        }
    );
    let after = store.owner(&renewing.token.agent_id).await.unwrap();
    store.database.drop().await.unwrap();
    eprintln!(
        "sustained_member_contention renewal_elapsed_ms={} result={renewed:?}",
        elapsed.as_millis()
    );
    aborted.unwrap();
    assert!(matches!(renewed, Err(OwnershipError::Unavailable(_))));
    assert!(elapsed >= Duration::from_millis(1_900));
    assert!(elapsed < Duration::from_millis(2_400));
    assert_eq!(
        after,
        Some(renewing),
        "failed renewal must not change its lease"
    );
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn four_owner_renewals_recover_after_a_shared_member_effect_commits() {
    let store = renewal_fixture().await;
    store.register(member("renewal-member")).await.unwrap();
    let effect = store
        .acquire(&AgentId("effect-agent".into()), "renewal-member")
        .await
        .unwrap()
        .unwrap();
    let mut owners = Vec::new();
    for index in 0..4 {
        owners.push(
            store
                .acquire(
                    &AgentId(format!("renewing-agent-{index}")),
                    "renewal-member",
                )
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .fence_transaction(&effect.token, &mut session)
        .await
        .unwrap();
    let (results, committed) = tokio::join!(
        futures_util::future::join_all(
            owners
                .iter()
                .map(|owner| bounded(store.renew(&owner.token)))
        ),
        async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            commit_transaction(&mut session).await
        }
    );
    store.database.drop().await.unwrap();
    assert!(matches!(committed, CommitOutcome::Committed));
    for (result, original) in results.into_iter().zip(owners) {
        assert_eq!(result.unwrap().token, original.token);
    }
}

//! A different Agent's bounded effect must not exhaust a live owner's renewal.
use super::*;
use layer2_core::agent_ownership::bounded;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn renewal_survives_bounded_contention_from_another_agents_effect() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("use the disposable Mongo integration harness");
    let database = std::env::var("CHRONO_TEST_MONGODB_DATABASE")
        .expect("declare the isolated disposable Mongo database");
    assert!(
        database.starts_with("cma_"),
        "use a disposable CMA database"
    );
    let client = Client::with_uri_str(uri).await.unwrap();
    let store = MongoAgentOwnership::open(&client, &database).await.unwrap();
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

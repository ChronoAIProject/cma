use super::*;
use crate::store::{
    agent_ownership::MongoAgentOwnership,
    sandbox_mongo_integration_tests::resource_access_tests::fixture,
};
use layer2_core::agent_ownership::PROTOCOL_VERSION;

async fn setup() -> (
    mongodb::Database,
    Arc<MongoAgentObservation>,
    layer2_core::sandbox::Agent,
) {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("observer_erasure_{}", uuid::Uuid::new_v4().simple());
    let (source, target) = fixture(&client, &name).await;
    let agent = source
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let coordinator = OwnershipCoordinator::register(
        Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap()),
        Member {
            instance_id: "observer".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap();
    let observer = MongoAgentObservation::open(Arc::new(source), coordinator)
        .await
        .unwrap();
    (client.database(&name), observer, agent)
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn old_snapshot_cannot_recreate_erased_observer_and_refresh_does_not_upsert() {
    let (db, observer, agent) = setup().await;
    let token = observer
        .destination(&agent.id, &agent.created_at)
        .await
        .unwrap()
        .authority
        .token
        .clone();
    let mut session = db.client().start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    // Pin a snapshot that still contains the source and live observer.
    assert!(observer
        .source
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .is_some());
    observer
        .source
        .agents
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    observer
        .leases
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let result = observer
        .acquire_in_session(&agent.id, &agent.created_at, "observer", &mut session)
        .await;
    assert!(matches!(result, Err(Error::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert!(observer
        .destination(&agent.id, &agent.created_at)
        .await
        .is_err());
    assert!(observer
        .refresh(std::slice::from_ref(&token))
        .await
        .unwrap()[0]
        .snapshot
        .is_none());
    assert!(observer.fresh(std::slice::from_ref(&token)).await.unwrap()[0].is_err());
    assert_eq!(observer.leases.count_documents(doc! {}).await.unwrap(), 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn delayed_acquisition_cannot_replace_a_new_agent_incarnation() {
    let (db, observer, agent) = setup().await;
    let mut replacement = agent.clone();
    replacement.created_at = "2026-11-20T00:00:00Z".into();
    observer
        .source
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&replacement).unwrap(),
        )
        .await
        .unwrap();
    assert!(observer
        .destination(&agent.id, &agent.created_at)
        .await
        .is_err());
    assert_eq!(observer.leases.count_documents(doc! {}).await.unwrap(), 0);
    let fresh = observer
        .destination(&replacement.id, &replacement.created_at)
        .await
        .unwrap();
    assert_eq!(fresh.authority.token.created_at, replacement.created_at);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn concurrent_readers_reuse_one_fenced_lease_without_exhausting_retries() {
    use futures_util::StreamExt;
    let (db, observer, agent) = setup().await;
    let tokens = futures_util::stream::iter(0..100)
        .map(|_| async {
            observer
                .destination(&agent.id, &agent.created_at)
                .await
                .unwrap()
                .authority
                .token
                .clone()
        })
        .buffer_unordered(32)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(tokens.len(), 100);
    assert!(tokens.iter().all(|token| token == &tokens[0]));
    assert_eq!(observer.leases.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        db.collection::<Document>(crate::store::agent_ownership::OWNERS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn unknown_source_version_aborts_the_guard_and_never_inserts_an_observer() {
    let (db, observer, agent) = setup().await;
    let mut original = observer
        .source
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    original.insert("document_schema_version", 99_i64);
    observer
        .source
        .agents
        .replace_one(doc! {"_id":agent.id.as_str()}, original.clone())
        .await
        .unwrap();
    assert!(observer
        .destination(&agent.id, &agent.created_at)
        .await
        .is_err());
    assert_eq!(
        observer
            .source
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap(),
        Some(original)
    );
    assert_eq!(observer.leases.count_documents(doc! {}).await.unwrap(), 0);
    db.drop().await.unwrap();
}

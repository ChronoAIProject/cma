use super::*;
use crate::store::{
    agent_observation::MongoAgentObservation, agent_ownership::MongoAgentOwnership,
};
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::agent_observation::*,
};

async fn manager(store: Arc<MongoAgentOwnership>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_read_leases_serialize_replicas_preserve_cadence_and_refuse_stale_epoch() {
    let (client, name) = database("observation_leases").await;
    let (source, target) = fixture(&client, &name).await;
    let source = Arc::new(source);
    let agent = source
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let native = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let a = manager(native.clone(), "observer-a").await;
    let b = manager(native, "observer-b").await;
    let first = MongoAgentObservation::open(source.clone(), a.clone())
        .await
        .unwrap();
    let second = MongoAgentObservation::open(source, b.clone())
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        first.destination(&agent.id, &agent.created_at),
        second.destination(&agent.id, &agent.created_at)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.authority.token, right.authority.token);
    let token = left.authority.token.clone();
    let (owner, peer, coordinator) = if token.instance_id == a.instance_id {
        (&first, &second, a)
    } else {
        (&second, &first, b)
    };
    assert!(
        owner.refresh(std::slice::from_ref(&token)).await.unwrap()[0]
            .snapshot
            .is_none()
    );
    assert!(peer.refresh(std::slice::from_ref(&token)).await.is_err());
    tokio::time::sleep(std::time::Duration::from_millis(1050)).await;
    let fresh = owner.refresh(std::slice::from_ref(&token)).await.unwrap();
    assert!(fresh[0].snapshot.as_ref().unwrap().is_ok());
    for _ in 0..3 {
        assert!(
            owner.refresh(std::slice::from_ref(&token)).await.unwrap()[0]
                .snapshot
                .is_none()
        );
    }
    let collection = client
        .database(&name)
        .collection::<Document>(crate::store::agent_observation::COLLECTION);
    collection
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$inc": {"epoch": 1_i64}},
        )
        .await
        .unwrap();
    assert!(owner.fresh(std::slice::from_ref(&token)).await.unwrap()[0].is_err());
    let stale = owner.refresh(std::slice::from_ref(&token)).await.unwrap();
    assert!(stale[0].snapshot.is_none());
    assert!(stale[0].until <= tokio::time::Instant::now());
    assert_eq!(
        client
            .database(&name)
            .collection::<Document>("cma_agent_manager_owners")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0,
        "reading history never acquires runtime ownership"
    );
    coordinator.fail().await;
    assert!(!owner.ready());
    assert!(owner.fresh(std::slice::from_ref(&token)).await.is_err());
    assert!(owner.refresh(std::slice::from_ref(&token)).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_refuses_unknown_lease_versions_and_replaced_agent_incarnations() {
    let (client, name) = database("observation_lease_refusal").await;
    let (source, target) = fixture(&client, &name).await;
    let source = Arc::new(source);
    let agent = source
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let native = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let coordinator = manager(native, "observer").await;
    let store = MongoAgentObservation::open(source, coordinator)
        .await
        .unwrap();
    let destination = store
        .destination(&agent.id, &agent.created_at)
        .await
        .unwrap();
    assert!(store
        .destination(&agent.id, "2000-01-01T00:00:00Z")
        .await
        .is_err());
    let collection = client
        .database(&name)
        .collection::<Document>(crate::store::agent_observation::COLLECTION);
    for (field, value) in [
        ("epoch", mongodb::bson::Bson::Int64(-1)),
        ("document_schema_version", mongodb::bson::Bson::Int64(9)),
    ] {
        let original = collection
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        collection
            .update_one(
                doc! {"_id": agent.id.as_str()},
                doc! {"$set": {field: value}},
            )
            .await
            .unwrap();
        assert!(store
            .destination(&agent.id, &agent.created_at)
            .await
            .is_err());
        assert!(store
            .fresh(std::slice::from_ref(&destination.authority.token))
            .await
            .unwrap()[0]
            .is_err());
        collection
            .replace_one(doc! {"_id": agent.id.as_str()}, original)
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn observation_owner_loss_waits_for_expiry_then_recovers_without_native_effects() {
    let (client, name) = database("observation_takeover").await;
    let (source, target) = fixture(&client, &name).await;
    let source = Arc::new(source);
    let agent = source
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let native = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let a = manager(native.clone(), "takeover-a").await;
    let b = manager(native, "takeover-b").await;
    let first = MongoAgentObservation::open(source.clone(), a.clone())
        .await
        .unwrap();
    let second = MongoAgentObservation::open(source.clone(), b.clone())
        .await
        .unwrap();
    let previous = first
        .destination(&agent.id, &agent.created_at)
        .await
        .unwrap();
    let (old, successor, failed, survivor) =
        if previous.authority.token.instance_id == a.instance_id {
            (&first, &second, a, b)
        } else {
            (&second, &first, b, a)
        };
    failed.fail().await;
    assert!(old
        .fresh(std::slice::from_ref(&previous.authority.token))
        .await
        .is_err());
    // The retained row prevents an eager takeover even after local failure.
    assert_eq!(
        successor
            .destination(&agent.id, &agent.created_at)
            .await
            .unwrap()
            .authority
            .token,
        previous.authority.token
    );
    for _ in 0..6 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        survivor.maintain().await.unwrap();
    }
    assert!(!previous.authority.current());
    let next = successor
        .destination(&agent.id, &agent.created_at)
        .await
        .unwrap();
    assert_eq!(next.authority.token.instance_id, survivor.instance_id);
    assert!(next.authority.token.epoch > previous.authority.token.epoch);
    assert!(successor
        .fresh(std::slice::from_ref(&next.authority.token))
        .await
        .unwrap()[0]
        .is_ok());
    assert!(successor
        .fresh(std::slice::from_ref(&previous.authority.token))
        .await
        .unwrap()[0]
        .is_err());
    assert!(successor
        .refresh(std::slice::from_ref(&next.authority.token))
        .await
        .unwrap()[0]
        .snapshot
        .is_none());
    source
        .agents
        .delete_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap();
    assert!(successor
        .fresh(std::slice::from_ref(&next.authority.token))
        .await
        .unwrap()[0]
        .is_err());
    assert_eq!(
        client
            .database(&name)
            .collection::<Document>("cma_agent_manager_owners")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

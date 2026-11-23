use super::super::super::test_support::MemoryOwnershipStore;
use super::super::super::PROTOCOL_VERSION;
use super::*;

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 256,
    }
}

fn agent(id: &str) -> AgentId {
    AgentId(id.into())
}

async fn registered(store: &MemoryOwnershipStore, id: &str) -> bool {
    store
        .membership()
        .await
        .unwrap()
        .members
        .iter()
        .any(|member| member.instance_id == id)
}

#[tokio::test]
async fn a_prepared_member_is_invisible_and_acquires_nothing_until_activated() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let starting = OwnershipCoordinator::prepare(store.clone(), member("starting"));
    assert!(!starting.is_registered());
    assert!(!starting.is_ready());
    assert!(!registered(&store, "starting").await);
    assert!(matches!(
        starting.acquire(&agent("agent")).await,
        Err(OwnershipError::Stale)
    ));
    // Placement sees only the serving peer, so no invitation targets startup.
    let serving = OwnershipCoordinator::register(store.clone(), member("serving"))
        .await
        .unwrap();
    let membership = store.membership().await.unwrap();
    assert_eq!(
        candidates(&agent("agent"), &membership, None).unwrap()[0].instance_id,
        "serving"
    );
    assert!(serving.acquire(&agent("agent")).await.unwrap().is_some());

    starting.activate().await.unwrap();
    assert!(starting.is_registered());
    assert!(starting.is_ready());
    assert!(registered(&store, "starting").await);
    // Once registered it takes its share of placement like any member.
    let membership = store.membership().await.unwrap();
    let preferred = (0..64)
        .map(|index| agent(&format!("agent-{index}")))
        .find(|id| candidates(id, &membership, None).unwrap()[0].instance_id == "starting")
        .expect("some Agent prefers the new member");
    assert!(starting.acquire(&preferred).await.unwrap().is_some());
    // Activation is idempotent once registered.
    starting.activate().await.unwrap();
}

#[tokio::test]
async fn admission_closed_before_activation_never_registers() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(store.clone(), member("terminating"));
    coordinator.stop_admission();
    assert!(matches!(
        coordinator.activate().await,
        Err(OwnershipError::Stale)
    ));
    assert!(!registered(&store, "terminating").await);
    assert!(coordinator.begin_drain().await.is_empty());
    assert!(!registered(&store, "terminating").await);
}

#[tokio::test]
async fn an_unknown_registration_outcome_fails_the_process() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::prepare(store.clone(), member("partitioned"));
    store.partition(true);
    assert!(matches!(
        coordinator.activate().await,
        Err(OwnershipError::Unavailable(_))
    ));
    assert!(*coordinator.failure_signal().borrow());
    store.partition(false);
    assert!(coordinator.activate().await.is_err());
    assert!(!coordinator.is_ready());
}

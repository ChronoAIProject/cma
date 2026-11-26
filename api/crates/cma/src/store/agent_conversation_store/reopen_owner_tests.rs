use crate::store::MongoSandboxStore;
use layer2_core::{
    agent_ownership::{
        candidates, observe_transport, transport_observation_scope, Authority, Member,
        OwnershipStore, PROTOCOL_VERSION,
    },
    sandbox::{
        store::{SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxStoreError},
        Agent,
    },
};
use std::{future::Future, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "requires a disposable local MongoDB replica set"]
async fn runtime_update_reopen_http_requires_the_callers_checkpoint_and_owner_fence() {
    crate::sandbox::exercise_runtime_update_reopen_http().await;
}

pub(super) async fn under<T>(owner: Option<&Arc<Authority>>, future: impl Future<Output = T>) -> T {
    match owner {
        Some(owner) => owner.run(future).await.unwrap(),
        None => future.await,
    }
}

pub(super) async fn setup(
    store: MongoSandboxStore,
    agent: &Agent,
    count: usize,
) -> (MongoSandboxStore, Option<Arc<Authority>>) {
    if count == 0 {
        return (store, None);
    }
    let store = store
        .enable_agent_ownership("reopen-test".into())
        .await
        .unwrap();
    let ownership = store.agent_ownership.as_ref().unwrap();
    for index in 0..count {
        ownership
            .register(Member {
                instance_id: format!("reopen-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: format!("http://reopen-{index}:8080"),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 256,
            })
            .await
            .unwrap();
    }
    let member = candidates(&agent.id, &ownership.membership().await.unwrap(), None).unwrap()[0]
        .instance_id
        .clone();
    let token = ownership
        .acquire(&agent.id, &member)
        .await
        .unwrap()
        .unwrap()
        .token;
    (
        store,
        Some(Authority::new(
            token,
            tokio::time::Instant::now() + Duration::from_secs(25),
        )),
    )
}

pub(super) async fn publish(
    store: &MongoSandboxStore,
    mutation: SandboxLifecycleMutation,
    initial: Option<&Arc<Authority>>,
) -> Option<Arc<Authority>> {
    let Some(initial) = initial else {
        store.commit_sandbox_lifecycle(mutation).await.unwrap();
        return None;
    };
    // Local transport authority alone cannot stand in for the exact epochs
    // returned by all remote reopen operations.
    assert_eq!(
        store
            .commit_sandbox_lifecycle(mutation.clone())
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    let ownership = store.agent_ownership.as_ref().unwrap();
    ownership.release(&initial.token).await.unwrap();
    ownership
        .heartbeat(&initial.token.instance_id, true)
        .await
        .unwrap();
    let member = candidates(
        &initial.token.agent_id,
        &ownership.membership().await.unwrap(),
        None,
    )
    .unwrap()[0]
        .instance_id
        .clone();
    let next = ownership
        .acquire(&initial.token.agent_id, &member)
        .await
        .unwrap()
        .unwrap()
        .token;
    assert_ne!(next.instance_id, initial.token.instance_id);
    assert!(next.epoch > initial.token.epoch);
    transport_observation_scope(async {
        observe_transport(initial.token.clone()).unwrap();
        assert_eq!(
            store
                .commit_sandbox_lifecycle(mutation.clone())
                .await
                .unwrap_err(),
            SandboxStoreError::Conflict
        );
    })
    .await;
    transport_observation_scope(async {
        observe_transport(next.clone()).unwrap();
        store.commit_sandbox_lifecycle(mutation).await.unwrap();
    })
    .await;
    Some(Authority::new(
        next,
        tokio::time::Instant::now() + Duration::from_secs(25),
    ))
}

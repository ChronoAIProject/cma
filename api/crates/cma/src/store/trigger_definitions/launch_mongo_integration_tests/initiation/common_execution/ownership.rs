use super::*;
use crate::store::{agent_ownership::MongoAgentOwnership, sandbox_mongo::MongoSandboxStore};
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn expired_trigger_refusal_projects_agent_only_under_current_durable_owner() {
    let accepted = Accepted::new().await;
    let client = &accepted.f.seed.client;
    let name = &accepted.f.seed.database;
    let owners = Arc::new(MongoAgentOwnership::open(client, name).await.unwrap());
    let store = MongoSandboxStore::open(client, name)
        .await
        .unwrap()
        .enable_agent_ownership("command-owner".into())
        .await
        .unwrap();
    let coordinator = OwnershipCoordinator::register(
        owners.clone(),
        Member {
            instance_id: "command-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://command-owner:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap();
    let authority = coordinator
        .acquire(&accepted.agent.id)
        .await
        .unwrap()
        .unwrap();
    let id = &accepted.command.receipt.envelope.command_id;
    let revision = accepted.command.revision;
    let now = &accepted.command.queue_deadline;
    let refuse = || {
        store.reject_unclaimed_command(id, revision, CommandFailureCode::QueueTimeout, now.clone())
    };
    assert_eq!(refuse().await, Err(CommandLedgerError::Conflict));
    assert_eq!(
        store.get_command(id).await.unwrap(),
        Some(accepted.command.clone())
    );
    assert_eq!(accepted.current_agent().await, accepted.agent);
    assert_eq!(
        authority
            .run(store.reject_unclaimed_command(
                id,
                revision + 1,
                CommandFailureCode::QueueTimeout,
                now.clone()
            ))
            .await
            .unwrap(),
        Err(CommandLedgerError::StaleClaim)
    );
    authority.run(refuse()).await.unwrap().unwrap();
    let command = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(command.receipt, accepted.command.receipt);
    assert_eq!(command.status, AgentResponseStatus::Failed);
    let agent = accepted.current_agent().await;
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Failed
    );
    assert_eq!(
        agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding,
        accepted
            .agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
    );
    assert!(agent.revision > accepted.agent.revision);
    assert!(command.work.dispatch.is_none());
    // A durable member fence rejects even a locally current token.
    owners.heartbeat("command-owner", true).await.unwrap();
    assert!(authority.is_current());
    assert_eq!(
        authority.run(store.reconcile_command(id)).await.unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    client.database(name).drop().await.unwrap();
}

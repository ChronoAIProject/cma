use super::*;
use crate::store::agent_capacity_recovery::RecoveryService;
use layer2_core::sandbox::agent_capacity_recovery::{
    ports::{RecoveryError, RecoveryLookup},
    state::RecoveryRequest,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_service_default_off_and_current_authority_preserve_source() {
    let (client, name) = database("capacity_service_authority").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "service").await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let foreign = Caller::new(
        SubjectId::new("foreign"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = RecoveryRequest {
        agent_id: agent.id.clone(),
        creation_key: agent.creation_key.clone(),
        creation_fingerprint: agent.creation_fingerprint.clone(),
        failure_seq: agent.conversation.last_seq,
        requested_at: NOW.into(),
    };
    let service = RecoveryService::new(store.clone(), transactions::access(&store));
    let before = store.agent_log(&agent.id, 0, 200).await.unwrap();
    assert!(matches!(
        service.lookup(&caller, &request).await.unwrap(),
        RecoveryLookup::Unknown
    ));
    assert_eq!(
        service.admit(&caller, &request).await,
        Err(RecoveryError::Disabled)
    );
    let status = service
        .status(&caller, &agent.id, Some(&request))
        .await
        .unwrap();
    assert!(status.phase.is_none());
    assert!(status.operation_key.is_none());
    assert!(matches!(
        service.status(&foreign, &agent.id, Some(&request)).await,
        Err(RecoveryError::Unauthorized)
    ));
    assert!(matches!(
        service.lookup(&foreign, &request).await,
        Err(RecoveryError::Unauthorized)
    ));
    assert!(matches!(
        service.admit(&foreign, &request).await,
        Err(RecoveryError::Unauthorized)
    ));
    // The service never retains the old authorization across requests.
    let mut changed = agent.clone();
    changed.owner = SubjectId::new("foreign");
    changed.revision += 1;
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            crate::store::agents::agent_document(&changed).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service.lookup(&caller, &request).await,
        Err(RecoveryError::Unauthorized)
    ));
    assert_eq!(store.agent_log(&agent.id, 0, 200).await.unwrap(), before);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(changed));
    client.database(&name).drop().await.unwrap();
}

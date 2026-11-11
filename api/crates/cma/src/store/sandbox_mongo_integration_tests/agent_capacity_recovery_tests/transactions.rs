use super::*;
use crate::store::agent_capacity_recovery::{records, settlement, transactions};
use layer2_core::resource_access::*;
use layer2_core::sandbox::agent_capacity_recovery::{
    ports::{RecoveryError, RecoveryLookup},
    state::RecoveryRequest,
};

struct PersonalOnly;
#[async_trait::async_trait]
impl OrganizationAuthority for PersonalOnly {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal recovery must not request external authority")
    }
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal recovery must not request external authority")
    }
}

async fn authority(store: &MongoSandboxStore, agent: &Agent) -> AuthorizedResource {
    ResourceAccessService::new(Arc::new(store.clone()), Arc::new(PersonalOnly))
        .authorize_personal(
            &Caller::new(
                agent.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            &ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            },
            ResourceCapability::OperateAgent,
        )
        .await
        .unwrap()
}

fn request(agent: &Agent) -> RecoveryRequest {
    RecoveryRequest {
        agent_id: agent.id.clone(),
        creation_key: agent.creation_key.clone(),
        creation_fingerprint: agent.creation_fingerprint.clone(),
        failure_seq: agent.conversation.last_seq,
        requested_at: NOW.into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_transaction_absence_is_unknown_across_restart_and_claim_aborts() {
    let (client, name) = database("capacity_root_absence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "receipt-absence").await;
    let request = request(&agent);
    let before = store.agent_log(&agent.id, 0, 200).await.unwrap();
    let authorized = authority(&store, &agent).await;
    assert!(matches!(
        transactions::lookup_accepted(&store, &authorized, &request)
            .await
            .unwrap(),
        RecoveryLookup::Unknown
    ));
    assert!(matches!(
        transactions::claim(
            &store,
            settlement::ClaimRequest {
                authorized: &authorized,
                request: &request,
                holder: "worker",
                now: NOW,
                expires_at: MINUTE,
            }
        )
        .await,
        Err(RecoveryError::Conflict)
    ));
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let fresh = authority(&reopened, &agent).await;
    assert!(matches!(
        transactions::lookup_accepted(&reopened, &fresh, &request)
            .await
            .unwrap(),
        RecoveryLookup::Unknown
    ));
    assert_eq!(
        reopened.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(reopened.agent_log(&agent.id, 0, 200).await.unwrap(), before);
    assert_eq!(
        records::collection(&reopened)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_transaction_authority_change_refuses_even_absent_receipt_lookup() {
    let (client, name) = database("capacity_root_owner").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_conversation_fixture(&store, "receipt-owner").await;
    let request = request(&agent);
    let authorized = authority(&store, &agent).await;
    agent.owner = SubjectId::new("changed-owner");
    agent.revision += 1;
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        transactions::lookup_accepted(&store, &authorized, &request).await,
        Err(RecoveryError::Conflict)
    ));
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        records::collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

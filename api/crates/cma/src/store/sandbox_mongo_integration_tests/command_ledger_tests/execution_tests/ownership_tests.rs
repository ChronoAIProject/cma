//! Production execution binds the explicit current resource owner.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_command_binding_checks_explicit_owner_kind_in_the_transaction() {
    let (client, name) = database("command_execution_owner_kind").await;
    let (store, agent, parent, execution, authority, sends) = composed(&client, &name).await;
    store
        .reserve_command(live_request(&agent, &parent, "one"))
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: parent.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    store.resource_policies.insert_one(doc! { "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION, "owner_kind": "organization", "owner_subject": parent.owner.as_str(), "revision": 1_i64, "record": crate::store::sandbox_mongo::record_bson(&policy).unwrap() }).await.unwrap();
    assert_eq!(
        execution.check_queued(&claimed).await,
        CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
    );
    // Bypass the adapter's earlier snapshot to prove the atomic store fence too.
    let caller = authority.0.lock().unwrap().clone().unwrap();
    assert_eq!(
        execution
            .operations
            .execute_claimed_command(&caller, &claimed)
            .await,
        Err(AgentOperationError::Conflict)
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
    client.database(&name).drop().await.unwrap();
}

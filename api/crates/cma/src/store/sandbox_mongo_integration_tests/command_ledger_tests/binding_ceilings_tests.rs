//! Validated binding execution receives the existing-consumer session share.
use super::*;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::ceilings_tests::configure;
use layer2_core::admission::{AdmissionCeilings, ExistingReserve};

fn policy(owner_limit: u32) -> AdmissionCeilings {
    AdmissionCeilings {
        active_sessions: Some(1),
        owner_active_sessions: Some(owner_limit),
        existing_reserve: ExistingReserve {
            active_sessions: Some(1),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bound_messages_use_existing_reserve_at_admission_and_start_but_keep_owner_limits() {
    let (client, name) = database("binding_session_cap").await;
    let (store, agent, parent, _, _, _) = composed(&client, &name).await;
    let (binding, _, _) = binding_for(&store, &agent).await;
    store
        .sandbox_admissions
        .insert_one(doc! {
            "_id": {"scope": "registration", "subject": agent.owner.as_str()},
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .await
        .unwrap();
    configure(&store, policy(1)).await;
    assert_eq!(
        store
            .reserve_command(live_request(&agent, &parent, "ordinary-public"))
            .await
            .unwrap_err(),
        CommandLedgerError::SessionCapacityUnavailable
    );
    let request = reservation(&store, &agent, &parent, &binding).await;
    configure(&store, policy(0)).await;
    assert_eq!(
        store.reserve_command(request.clone()).await.unwrap_err(),
        CommandLedgerError::SessionCapacityUnavailable
    );
    configure(&store, policy(1)).await;
    store.reserve_command(request).await.unwrap();
    let command = store
        .claim_next_command(claim_request(
            &agent,
            "binding-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let auth = auth(store.clone());
    let access = ResourceAccessService::new(store.clone(), auth.clone())
        .with_bindings(BindingAccessService::new(store.clone()));
    let proof = auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .unwrap();
    assert!(proof.fence().binding_authority().is_some());
    let mut mutation =
        message_mutation(&agent, &parent, &command.work.lease.as_ref().unwrap().claim);
    mutation.execution_fence = Some(proof.fence());
    configure(&store, policy(0)).await;
    assert_eq!(
        store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap_err(),
        SandboxStoreError::SessionCapacityUnavailable
    );
    configure(&store, policy(1)).await;
    store.commit_agent_conversation(mutation).await.unwrap();
    assert_eq!(
        store
            .agents
            .count_documents(doc! {"status":"working"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"record.kind.type":"turn_started"})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::store::SandboxStore;

async fn request(a: &Accepted) -> QueuedCancellationReservation {
    let parent =
        a.f.resources
            .get_sandbox(&a.agent.sandbox_id)
            .await
            .unwrap()
            .unwrap();
    let mut command = crate::store::sandbox_mongo_integration_tests::command_ledger_tests::work_tests::live_request(
        &a.agent, &parent, "cancel-segmented-trigger",
    );
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    command.admission_audit = Some(ResourceAuthorizationAudit {
        target: ResourceTarget::Agent {
            agent_id: a.agent.id.clone(),
        },
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
        actor: ResourceActor::Human {
            subject: a.agent.owner.clone(),
        },
        policy_revision: 0,
        capability: ResourceCapability::OperateAgent,
        captured_at: command.admitted_at.clone(),
        observations: vec![ResourceAuthorityObservation {
            capabilities: vec![ResourceCapability::OperateAgent],
            basis: ResourceAuthorizationBasis::PersonalOwner,
        }],
    });
    QueuedCancellationReservation {
        command,
        target: QueuedCancellationTarget {
            response_id: a.command.receipt.response_id.clone().unwrap(),
            source_command_id: a.command.receipt.envelope.command_id.clone(),
            expected_command_revision: a.command.revision,
        },
        observed_agent_revision: a.agent.revision,
        agent_created_at: a.agent.created_at.clone(),
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_queued_cancellation_keeps_control_and_terminal_publication_atomic() {
    let a = accepted().await;
    let request = request(&a).await;
    let result = a.f.resources.cancel_queued_response(request).await.unwrap();
    let QueuedCancellationOutcome::Applied(result) = result else {
        panic!("not admitted")
    };
    assert_eq!(result.response.receipt, a.command.receipt);
    assert_eq!(result.response.status, AgentResponseStatus::Cancelled);
    assert!(result.response.work.dispatch.is_none());
    let agent = a.current_agent().await;
    assert_eq!(
        agent.conversation.last_seq,
        a.agent.conversation.last_seq + 1
    );
    assert_eq!(
        agent.conversation.first_message.unwrap().state,
        FirstMessageDelivery::Failed
    );
    assert_eq!(a.current_command().await, result.response);
    assert_eq!(
        a.f.resources
            .get_command(&result.record.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(result.record)
    );
    a.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_queued_cancellation_reconciliation_requires_original_control_artifact() {
    let a = accepted().await;
    let request = request(&a).await;
    let mut session = a.f.seed.client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let (result, receipt) = match command_worker::cancel(&a.f.resources, &request, &mut scope).await
    {
        Ok(value) => value,
        Err(_) => panic!("cancellation refused"),
    };
    let receipt = receipt.unwrap();
    scope.release().commit_transaction().await.unwrap();
    append(&a.f, &a.agent, "after-cancellation").await;
    a.f.resources
        .agents
        .find_one(doc! {"_id":a.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    assert!(
        terminal::reconcile(&a.f.resources, Some(&receipt), session.operation_time())
            .await
            .unwrap()
    );
    let QueuedCancellationOutcome::Applied(result) = result else {
        panic!("not admitted")
    };
    a.f.resources
        .agent_commands
        .delete_one(doc! {"_id":result.record.receipt.envelope.command_id.as_str()})
        .await
        .unwrap();
    a.f.resources
        .agents
        .find_one(doc! {"_id":a.agent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    assert!(
        !terminal::reconcile(&a.f.resources, Some(&receipt), session.operation_time())
            .await
            .unwrap()
    );
    a.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_queued_cancellation_reconciles_lost_commit_acknowledgement() {
    let mut a = accepted().await;
    let request = request(&a).await;
    let app = unknown::isolate(&mut a.f).await;
    unknown::lose_ack(&a.f, &app).await;
    let result = a.f.resources.cancel_queued_response(request).await;
    unknown::clear(&a.f).await;
    let QueuedCancellationOutcome::Applied(result) = result.unwrap() else {
        panic!("not admitted")
    };
    assert_eq!(a.current_command().await, result.response);
    assert_eq!(
        a.current_agent().await.conversation.last_seq,
        a.agent.conversation.last_seq + 1
    );
    assert_eq!(
        a.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    a.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_queued_cancellation_rolls_back_its_control_when_publication_refuses() {
    let a = accepted().await;
    let request = request(&a).await;
    a.f.seed
        .client
        .database(&a.f.seed.database)
        .collection::<Document>("agent_log_heads")
        .delete_one(doc! {"_id":a.agent.id.as_str()})
        .await
        .unwrap();
    assert!(a.f.resources.cancel_queued_response(request).await.is_err());
    assert_eq!(a.current_agent().await, a.agent);
    assert_eq!(a.current_command().await, a.command);
    assert_eq!(
        a.f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    a.f.close().await;
}

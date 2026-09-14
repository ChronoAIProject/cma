use std::sync::Arc;

use super::fixtures::*;
use crate::sandbox::agent_io::{AgentAttachmentReference, AgentCommand, AgentContent, AgentInput};
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;

async fn setup(
    status: AgentStatus,
) -> (
    Arc<InMemorySandboxStore>,
    Arc<AgentOperationService>,
    Agent,
    Arc<std::sync::Mutex<Vec<layer3_adapter::ConversationInput>>>,
) {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let agent = test_agent(status, None);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry);
    (store, service, agent, inputs)
}

#[tokio::test]
async fn typed_and_legacy_input_use_the_same_fences_logs_and_runtime_bytes() {
    let (legacy_store, legacy, old_agent, old_inputs) = setup(AgentStatus::Ready).await;
    let (typed_store, typed, agent, inputs) = setup(AgentStatus::Ready).await;
    let old_result = legacy
        .send_message(&owner(), &old_agent.id, "Inspect this.".into())
        .await
        .unwrap();
    let result = typed
        .execute_command(&owner(), &agent.id, AgentCommand::message("Inspect this."))
        .await
        .unwrap();
    let (AgentMessageOutcome::Dispatched(old), AgentMessageOutcome::Dispatched(new)) =
        (old_result, result)
    else {
        panic!("expected dispatched commands")
    };
    assert_eq!(old.operation_sequence, new.operation_sequence);
    assert_eq!(old.turn_sequence, new.turn_sequence);
    assert_eq!(old.delivery, new.delivery);
    for sent in [&old_inputs, &inputs] {
        assert!(
            matches!(sent.lock().unwrap().as_slice(), [layer3_adapter::ConversationInput::Prompt(content)] if prompt_text(content) == "Inspect this.")
        );
    }
    let old_log = legacy_store.agent_log(&agent.id, 0, 20).await.unwrap();
    let new_log = typed_store.agent_log(&agent.id, 0, 20).await.unwrap();
    assert_eq!(
        old_log
            .iter()
            .map(|e| (&e.agent_id, e.seq, &e.kind))
            .collect::<Vec<_>>(),
        new_log
            .iter()
            .map(|e| (&e.agent_id, e.seq, &e.kind))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn typed_commands_refuse_foreign_agents_and_missing_authority_before_runtime() {
    let (store, service, agent, inputs) = setup(AgentStatus::Ready).await;
    for (caller, id, expected) in [
        (
            unauthorized_owner(),
            agent.id.clone(),
            AgentOperationError::Forbidden,
        ),
        (
            other_owner(),
            agent.id.clone(),
            AgentOperationError::NotFound,
        ),
        (
            owner(),
            crate::AgentId("agt_absent".into()),
            AgentOperationError::NotFound,
        ),
    ] {
        assert_eq!(
            service
                .execute_command(&caller, &id, AgentCommand::message("hello"))
                .await
                .unwrap_err(),
            expected
        );
    }
    assert!(inputs.lock().unwrap().is_empty());
    assert!(store.agent_log(&agent.id, 0, 20).await.unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
}

#[tokio::test]
async fn unsupported_attachment_does_not_admit_partial_text() {
    let (store, service, agent, inputs) = setup(AgentStatus::Ready).await;
    let command = AgentCommand::Steer {
        input: AgentInput {
            content: vec![
                AgentContent::Text {
                    text: "Inspect the attachment".into(),
                },
                AgentContent::Attachment {
                    reference: AgentAttachmentReference {
                        id: "attachment-one".into(),
                        sha256: "a".repeat(64),
                        media_type: "text/plain".into(),
                        size_bytes: 12,
                    },
                },
            ],
        },
    };
    assert_eq!(
        service
            .execute_command(&owner(), &agent.id, command)
            .await
            .unwrap_err(),
        AgentOperationError::ContentUnsupported
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
}

#[tokio::test]
async fn typed_control_keeps_the_existing_approval_and_dispatch_fences() {
    let (store, service, agent, inputs) = setup(AgentStatus::Working).await;
    let outcome = service
        .execute_command(&owner(), &agent.id, AgentCommand::Interrupt)
        .await
        .unwrap();
    assert!(matches!(outcome, AgentMessageOutcome::Dispatched(_)));
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [layer3_adapter::ConversationInput::Cancel]
    ));
    assert!(matches!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .operation
            .unwrap()
            .intent,
        AgentOperationIntent::Interrupt
    ));

    let store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&store).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .execute_command(
            &owner(),
            &waiting.id,
            AgentCommand::AnswerApproval {
                approval_id: APPROVAL_ID.into(),
                decision: AgentApprovalDecision::Allow,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [layer3_adapter::ConversationInput::PermissionAnswer { .. }]
    ));
    assert!(store.agent_log(&waiting.id, 0, 20).await.unwrap().iter().any(|entry| matches!(&entry.kind, AgentLogKind::ApprovalResolved { approval_id, decision } if approval_id == APPROVAL_ID && decision == "allow")));
}

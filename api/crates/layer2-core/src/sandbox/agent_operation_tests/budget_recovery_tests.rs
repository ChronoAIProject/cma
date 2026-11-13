use std::sync::{Arc, Mutex};

use super::super::command_work_tests::{admit, fixture, request};
use super::super::fixtures::*;
use crate::budget::{BudgetError, BudgetStop, SpendSource};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;
use layer3_adapter::{
    ContentChunk, ConversationUpdate, PromptIdentity, PromptResponse, SessionUpdate, TurnStopReason,
};

struct Source(Mutex<Option<BudgetStop>>);

#[async_trait::async_trait]
impl SpendSource for Source {
    async fn stop_requested(
        &self,
        _: &crate::AgentId,
        _: u64,
        _: u64,
    ) -> Result<Option<BudgetStop>, BudgetError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn budget_stop_waits_for_terminal_evidence_and_preserves_the_failed_receipt() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "budget-original",
        AgentCommand::message("Begin"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let work = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn = working.conversation.active_turn.unwrap();
    let source = Arc::new(Source(Mutex::new(Some(BudgetStop {
        agent_id: agent.id.clone(),
        runtime_generation: 3,
        turn_sequence: turn,
        reason: BudgetError::TurnExhausted,
    }))));
    let recorder = DurableAgentUpdateRecorder::with_spend_source(
        store.clone(),
        Arc::default(),
        Some(source.clone()),
    );
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &agent.id,
            3,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                "Completed output.".into(),
            ))),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(recorder.budget_stop(&agent.id, 3).await.unwrap().is_some());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            working.conversation.conversation_id.as_deref().unwrap(),
            3,
            turn,
        ),
        response: PromptResponse::new(TurnStopReason::Cancelled),
    };
    let outcome = recorder
        .record_update(&agent.id, 3, &mut assembly, terminal, crate::now_rfc3339())
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let failed = store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(failed.receipt, admitted.receipt);
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    let replay = admit(
        &store,
        &ready,
        &parent,
        "budget-original",
        AgentCommand::message("Begin"),
    )
    .await;
    assert_eq!(replay.receipt, admitted.receipt);
    assert_eq!(replay.status, AgentResponseStatus::Failed);
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert!(history.iter().any(
        |entry| matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. }
        if failure.class == "turn_budget_exhausted" && failure.detail.contains("HTTP 402"))
    ));
    assert!(history.iter().any(
        |entry| matches!(&entry.kind, AgentLogKind::AgentMessage { content }
        if content == "Completed output.")
    ));
    *source.0.lock().unwrap() = None;
    let next = admit(
        &store,
        &ready,
        &parent,
        "budget-continuation",
        AgentCommand::message("Continue with less scope"),
    )
    .await;
    let work = store
        .claim_next_command(request(&ready, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    recorder
        .record_update(
            &agent.id,
            3,
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .reconcile_command(&next.receipt.envelope.command_id)
            .await
            .unwrap()
            .status,
        AgentResponseStatus::Completed
    );
    assert_eq!(
        store
            .reconcile_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .status,
        AgentResponseStatus::Failed
    );
    assert_eq!(inputs.lock().unwrap().len(), 2);
}

#[test]
fn unattempted_first_message_budget_refusal_is_terminal_and_not_replayable() {
    let mut agent = test_agent(AgentStatus::Ready, Some("Begin"));
    let first = agent.conversation.first_message.as_mut().unwrap();
    first
        .refuse_budget(BudgetError::OwnerExhausted, LATER.into())
        .unwrap();
    first.validate().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(first.turn_sequence, None);
    assert_eq!(
        first.failure.as_ref().unwrap().code,
        "owner_budget_exhausted"
    );
    assert!(!first.failure.as_ref().unwrap().retryable);
    assert!(first
        .authorize_recovery(&first.payload_sha256.clone(), true, LATEST.into())
        .is_err());
}

#[tokio::test]
async fn terminal_evidence_after_a_platform_stop_fails_the_turn_with_platform_budget_exhausted() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "platform-original",
        AgentCommand::message("Begin"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let work = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    service
        .execute_claimed_command(&owner(), &work)
        .await
        .unwrap();
    let working = store.get_agent(&agent.id).await.unwrap().unwrap();
    let turn = working.conversation.active_turn.unwrap();
    let source = Arc::new(Source(Mutex::new(Some(BudgetStop {
        agent_id: agent.id.clone(),
        runtime_generation: 3,
        turn_sequence: turn,
        reason: BudgetError::PlatformExhausted,
    }))));
    let recorder = DurableAgentUpdateRecorder::with_spend_source(
        store.clone(),
        Arc::default(),
        Some(source.clone()),
    );
    let mut assembly = AgentMessageAssembly::default();
    assert!(recorder.budget_stop(&agent.id, 3).await.unwrap().is_some());
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            working.conversation.conversation_id.as_deref().unwrap(),
            3,
            turn,
        ),
        response: PromptResponse::new(TurnStopReason::Cancelled),
    };
    assert!(
        recorder
            .record_update(&agent.id, 3, &mut assembly, terminal, crate::now_rfc3339())
            .await
            .unwrap()
            .turn_closed
    );
    let failed = store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(failed.receipt, admitted.receipt);
    let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert!(history.iter().any(
        |entry| matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. }
        if failure.class == "platform_budget_exhausted"
            && failure.detail.contains("platform's spend limit"))
    ));
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[test]
fn unattempted_first_message_platform_refusal_is_terminal_and_not_replayable() {
    let mut agent = test_agent(AgentStatus::Ready, Some("Begin"));
    let first = agent.conversation.first_message.as_mut().unwrap();
    first
        .refuse_budget(BudgetError::PlatformExhausted, LATER.into())
        .unwrap();
    first.validate().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(
        first.failure.as_ref().unwrap().code,
        "platform_budget_exhausted"
    );
    assert!(!first.failure.as_ref().unwrap().retryable);
}

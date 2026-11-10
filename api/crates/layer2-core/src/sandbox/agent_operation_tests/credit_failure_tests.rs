use layer3_adapter::{
    ContentChunk, ConversationUpdate, PromptIdentity, PromptResponse, SessionUpdate, ToolCall,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, TurnStopReason,
};

use super::command_work_tests::{admit, fixture, request};
use super::fixtures::*;
use crate::sandbox::agent_io::*;
use crate::sandbox::conversation_fold::{fold_update, FoldedConversationUpdate};
use crate::sandbox::store::{AgentLogStore, AgentStore, SandboxStore};
use crate::sandbox::*;

const DETAIL: &str = "HTTP 402: insufficient model credits. Top up, then send a new message.";

fn refusal(agent: &Agent) -> ConversationUpdate {
    ConversationUpdate::FailedForPrompt {
        prompt_id: PromptIdentity::new(
            agent.conversation.conversation_id.as_deref().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            agent.conversation.active_turn.unwrap(),
        ),
        message: DETAIL.into(),
        capability: Some("insufficient_credits".into()),
    }
}

#[test]
fn first_message_credit_failure_retains_its_type_and_cannot_be_replayed() {
    let mut agent = test_agent(AgentStatus::Working, Some("Begin the task."));
    agent
        .conversation
        .first_message
        .as_mut()
        .unwrap()
        .fence_dispatch_for(ACTIVE_TURN, 3, 1, LATER.into())
        .unwrap();
    let FoldedConversationUpdate::Changed(change) = fold_update(
        &agent,
        &AgentMessageAssembly::default(),
        refusal(&agent),
        LATEST,
        false,
    )
    .unwrap() else {
        panic!("credit refusal must settle the admitted turn")
    };
    assert!(change.turn_closed);
    assert_eq!(change.agent.status, AgentStatus::Ready);
    assert_eq!(change.agent.conversation.active_turn, None);
    let mut first = change.agent.conversation.first_message.unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(first.failure.as_ref().unwrap().code, "insufficient_credits");
    assert!(!first.failure.as_ref().unwrap().retryable);
    assert!(first
        .authorize_recovery(&first.payload_sha256.clone(), true, LATEST.into())
        .is_err());
    assert!(change.logs.iter().any(|kind| matches!(kind,
        AgentLogKind::TurnFailed { failure, .. }
        if failure.class == "insufficient_credits" && failure.detail == DETAIL)));
}

#[tokio::test]
async fn credit_failure_keeps_receipt_and_effects_and_allows_explicit_continuation() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "original",
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
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    // Preserve output already produced before a later model call was refused.
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        SessionUpdate::ToolCall(
            ToolCall::new("effect", "Write task output").status(ToolCallStatus::InProgress),
        ),
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "effect",
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        )),
        SessionUpdate::AgentMessageChunk(ContentChunk::new("Earlier work is preserved.".into())),
    ] {
        recorder
            .record_update(
                &agent.id,
                3,
                &mut assembly,
                ConversationUpdate::Said(update),
                crate::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let outcome = recorder
        .record_update(
            &agent.id,
            3,
            &mut assembly,
            refusal(&working),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let id = &admitted.receipt.envelope.command_id;
    let failed = store.reconcile_command(id).await.unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(failed.receipt, admitted.receipt);
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    let parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let replay = admit(
        &store,
        &ready,
        &parent,
        "original",
        AgentCommand::message("Begin"),
    )
    .await;
    assert_eq!(replay.receipt, admitted.receipt);
    assert_eq!(replay.status, AgentResponseStatus::Failed);
    assert!(store
        .claim_next_command(request(&ready, "replacement", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert!(history.iter().any(|entry| matches!(&entry.kind,
        AgentLogKind::ToolCallUpdated { call_id, status, .. }
        if call_id == "effect" && status == "completed")));
    assert!(history.iter().any(|entry| matches!(&entry.kind,
        AgentLogKind::AgentMessage { content } if content == "Earlier work is preserved.")));
    assert!(history.iter().any(|entry| matches!(&entry.kind,
        AgentLogKind::TurnFailed { failure, .. } if failure.class == "insufficient_credits")));

    // Restored funding is represented by the next explicit request succeeding.
    // A top-up itself must not enqueue or replay any old command.
    let next = admit(
        &store,
        &ready,
        &parent,
        "continuation",
        AgentCommand::message("Continue"),
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
        store.reconcile_command(id).await.unwrap().status,
        AgentResponseStatus::Failed
    );
    let restored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        restored.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(restored.worktree_id, agent.worktree_id);
    assert_eq!(inputs.lock().unwrap().len(), 2);
    let after = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(&after[..history.len()], history.as_slice());
}

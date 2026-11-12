//! Contextual and targeted admissions after genuine legacy-to-v2 publication.
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::{
    AgentApprovalDecision, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PermissionLocator, PermissionRequest, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use std::sync::Arc;
mod support;
use support::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_contextual_admission_uses_original_answer_and_refuses_wrong_sequence() {
    let (client, name, store, working, source) = active("v2_answer").await;
    let turn = working.conversation.active_turn.unwrap();
    let answer = store
        .append_agent_log(
            &working.id,
            NOW.into(),
            AgentLogKind::AgentMessage {
                content: "Original answer.".into(),
            },
        )
        .await
        .unwrap();
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    store
        .reconcile_command(&source.receipt.envelope.command_id)
        .await
        .unwrap();
    let ready = store.get_agent(&working.id).await.unwrap().unwrap();
    let adopted = adopt(&client, &name, &store, ready).await;
    let parent = store
        .get_sandbox(&adopted.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut request = live_request(&adopted, &parent, "context");
    AnswerContext {
        turn_sequence: turn,
        answer_sequence: answer.seq,
    }
    .bind(&mut request.envelope.command)
    .unwrap();
    request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
    let mut wrong = live_request(&adopted, &parent, "wrong-context");
    AnswerContext {
        turn_sequence: turn,
        answer_sequence: answer.seq + 1,
    }
    .bind(&mut wrong.envelope.command)
    .unwrap();
    wrong.envelope.request_fingerprint = wrong.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_command(wrong).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    let accepted = store.reserve_command(request.clone()).await.unwrap();
    assert_eq!(
        record(store.reserve_command(request).await.unwrap()).receipt,
        record(accepted).receipt
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":adopted.id.as_str()})
            .await
            .unwrap(),
        adopted.conversation.last_seq
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_targeted_approval_reads_indexed_request_and_replays_exact_receipt() {
    let (client, name, store, working, source) = active("v2_approval").await;
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Asked(PermissionRequest {
                id: "permission-exact".into(),
                locator: Some(PermissionLocator::new("private-runtime-locator")),
                request: RequestPermissionRequest::new(
                    "conversation",
                    ToolCallUpdate::new("call", ToolCallUpdateFields::new().title("Read file")),
                    vec![],
                ),
            }),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let waiting = store.get_agent(&working.id).await.unwrap().unwrap();
    let adopted = adopt(&client, &name, &store, waiting).await;
    let parent = store
        .get_sandbox(&adopted.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut command = live_request(&adopted, &parent, "target");
    command.envelope.command = AgentCommand::AnswerApproval {
        approval_id: "permission-exact".into(),
        decision: AgentApprovalDecision::Allow,
    };
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    let request = TargetedControlReservation {
        command,
        target: TargetedResponse {
            response_id: source.receipt.response_id.unwrap(),
            turn_sequence: working.conversation.active_turn.unwrap(),
        },
        observed_agent_revision: adopted.revision,
        agent_created_at: adopted.created_at.clone(),
        retry_deadline: None,
    };
    let mut wrong = request.clone();
    wrong.command.envelope.command = AgentCommand::AnswerApproval {
        approval_id: "foreign".into(),
        decision: AgentApprovalDecision::Allow,
    };
    wrong.command.envelope.request_fingerprint =
        wrong.command.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_targeted_control(wrong).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    let accepted = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        store.lookup_targeted_control(request).await.unwrap(),
        Some(accepted)
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":adopted.id.as_str()})
            .await
            .unwrap(),
        adopted.conversation.last_seq
    );
    client.database(&name).drop().await.unwrap();
}

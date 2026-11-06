use super::*;
use layer2_core::sandbox::{
    AgentApprovalDecision, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PermissionLocator, PermissionRequest, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn approval_uses_complete_pending_snapshot_and_replay_survives_resolution() {
    let (client, name) = database("targeted_approval").await;
    let (store, agent, mut request, source) = composed(&name, &client).await;
    DurableAgentUpdateRecorder::new(std::sync::Arc::new(store.clone()))
        .record_update(
            &agent.id,
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
    let waiting = store.get_agent(&agent.id).await.unwrap().unwrap();
    request.observed_agent_revision = waiting.revision;
    request.command.expected_agent_revision = waiting.revision;
    request.command.envelope.command = AgentCommand::AnswerApproval {
        approval_id: "permission-exact".into(),
        decision: AgentApprovalDecision::Allow,
    };
    request.command.envelope.request_fingerprint =
        request.command.envelope.command.fingerprint().unwrap();
    let audit = request.command.admission_audit.as_mut().unwrap();
    audit.capability = layer2_core::resource_access::ResourceCapability::ResolveApproval;
    audit.observations[0].capabilities = vec![audit.capability];
    let mut wrong = request.clone();
    wrong.command.envelope.command = AgentCommand::AnswerApproval {
        approval_id: "missing-permission".into(),
        decision: AgentApprovalDecision::Allow,
    };
    wrong.command.envelope.request_fingerprint =
        wrong.command.envelope.command.fingerprint().unwrap();
    let before = snapshot(&store).await;
    assert_eq!(
        store.reserve_targeted_control(wrong).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    assert_eq!(snapshot(&store).await, before);
    let original = store
        .reserve_targeted_control(request.clone())
        .await
        .unwrap();
    // Resolve the pending approval through a complete ordinary conversation
    // mutation before completing its turn. WaitingApproval cannot jump to Ready.
    let at = layer2_core::now_rfc3339();
    let mut resumed = waiting.clone();
    resumed
        .transition(AgentStatus::Working, at.clone(), None)
        .unwrap();
    resumed.conversation.last_seq += 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            reconciliation_fence: None,
            expected_agent_revision: waiting.revision,
            expected_last_seq: waiting.conversation.last_seq,
            runtime_generation: 1,
            agent: resumed.clone(),
            appended_at: at,
            agent_logs: vec![AgentLogKind::ApprovalResolved {
                approval_id: "permission-exact".into(),
                decision: "allow".into(),
            }],
        })
        .await
        .unwrap();
    let next = finish_and_start(&store, &resumed, &source).await;
    request.observed_agent_revision = next.revision;
    assert_eq!(
        store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap(),
        original
    );
    let mut fresh = request.clone();
    fresh.command.envelope.command_id = AgentCommandId::generate();
    fresh.command.key_sha256 = command_key_digest("new-approval-after-resolution").unwrap();
    fresh.command.expected_agent_revision = next.revision;
    assert_eq!(
        store.reserve_targeted_control(fresh).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn approval_refuses_a_gapped_log_without_any_reservation_effect() {
    let (client, name) = database("targeted_approval_gap").await;
    let (store, _, mut request, _) = composed(&name, &client).await;
    request.command.envelope.command = AgentCommand::AnswerApproval {
        approval_id: "permission".into(),
        decision: AgentApprovalDecision::Allow,
    };
    request.command.envelope.request_fingerprint =
        request.command.envelope.command.fingerprint().unwrap();
    let audit = request.command.admission_audit.as_mut().unwrap();
    audit.capability = layer2_core::resource_access::ResourceCapability::ResolveApproval;
    audit.observations[0].capabilities = vec![audit.capability];
    store
        .agent_log
        .delete_one(doc! { "agent_id": request.command.envelope.agent_id.as_str(), "seq": 2_i64 })
        .await
        .unwrap();
    let before = snapshot(&store).await;
    assert_eq!(
        store.reserve_targeted_control(request).await.unwrap_err(),
        CommandLedgerError::Corrupt
    );
    assert_eq!(snapshot(&store).await, before);
    client.database(&name).drop().await.unwrap();
}

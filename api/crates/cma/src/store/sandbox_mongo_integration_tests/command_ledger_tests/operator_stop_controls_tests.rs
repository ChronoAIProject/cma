//! Controls on admitted work remain available during an operator stop.
use super::*;
use layer2_core::admission::AdmissionControlStore;
use layer2_core::sandbox::{
    AgentApprovalDecision, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PermissionLocator, PermissionRequest, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};

pub(super) async fn prepare_approval(
    store: &MongoSandboxStore,
    agent: &Agent,
    request: &mut TargetedControlReservation,
) {
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Asked(PermissionRequest {
                id: "pending-approval".into(),
                locator: Some(PermissionLocator::new("private-control-locator")),
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
        approval_id: "pending-approval".into(),
        decision: AgentApprovalDecision::Allow,
    };
    request.command.envelope.request_fingerprint =
        request.command.envelope.command.fingerprint().unwrap();
    let audit = request.command.admission_audit.as_mut().unwrap();
    audit.capability = layer2_core::resource_access::ResourceCapability::ResolveApproval;
    audit.observations[0].capabilities = vec![audit.capability];
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_preserves_interrupt_and_approval_admission_and_exact_replay() {
    for approval in [false, true] {
        let (client, name) = database("operator_controls").await;
        let (store, agent, mut request, _) = composed(&name, &client).await;
        crate::store::admission_control::MongoAdmissionControlStore::open(&client, &name)
            .await
            .unwrap()
            .change(
                0,
                &SubjectId::new("operator"),
                "contain new work",
                true,
                100,
            )
            .await
            .unwrap();
        if approval {
            prepare_approval(&store, &agent, &mut request).await;
        }
        let accepted = store
            .reserve_targeted_control(request.clone())
            .await
            .unwrap();
        assert_eq!(
            store.reserve_targeted_control(request).await.unwrap(),
            accepted
        );
        assert_eq!(
            store
                .agent_commands
                .count_documents(doc! {"explicit_control": {"$exists": true}})
                .await
                .unwrap(),
            1
        );
        client.database(&name).drop().await.unwrap();
    }
}

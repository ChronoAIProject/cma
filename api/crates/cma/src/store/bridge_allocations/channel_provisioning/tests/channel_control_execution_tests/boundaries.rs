use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_source_completion_and_reset_cannot_select_another_target() {
    for reset in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, true, "source-boundary-control").await;
        if reset {
            let (admission, authority, management) = proposal(
                &c.f.svc,
                &c.f.bridges,
                &c.f.allocation.pin.bridge_id,
                "12345",
                Some(&c.f.allocation),
            )
            .await;
            c.f.allocations
                .reserve(
                    AllocationReservation::reset(
                        admission,
                        authority,
                        c.f.allocation.clone(),
                        management,
                        now(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        } else {
            c.f.finish().await;
            c.f.resources
                .reconcile_command(&c.f.command.receipt.envelope.command_id)
                .await
                .unwrap();
            layer2_core::sandbox::AgentOperationService::new(
                c.f.resources.clone(),
                c.f.registry.clone(),
            )
            .execute_command(
                &caller().caller,
                &held.command.receipt.envelope.agent_id,
                AgentCommand::message("a later authenticated owner's turn"),
            )
            .await
            .unwrap();
        }
        let (execution, writes) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution).await;
        assert!(writes.lock().unwrap().is_empty());
        assert!(c
            .f
            .resources
            .get_command(&held.sidecar.command_id)
            .await
            .unwrap()
            .unwrap()
            .work
            .dispatch
            .is_none());
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_pending_approval_never_resolves_approval_or_steers_through_it() {
    for steer in [true, false] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, steer, "pending-approval-control").await;
        let agent =
            c.f.resources
                .get_agent(&held.command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
        DurableAgentUpdateRecorder::new(c.f.resources.clone())
            .record_update(
                &agent.id,
                agent.conversation.runtime_generation.unwrap(),
                &mut AgentMessageAssembly::default(),
                layer3_adapter::ConversationUpdate::Asked(layer3_adapter::PermissionRequest {
                    id: "channel-dashboard-only".into(),
                    locator: Some(layer3_adapter::PermissionLocator::new(
                        "owned-permission-locator",
                    )),
                    request: layer3_adapter::RequestPermissionRequest::new(
                        agent.conversation.conversation_id.unwrap(),
                        layer3_adapter::ToolCallUpdate::new(
                            "owned-call",
                            layer3_adapter::ToolCallUpdateFields::new().title("Read file"),
                        ),
                        vec![],
                    ),
                }),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        c.f.resources
            .reconcile_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap();
        let (execution, writes) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution).await;
        let current_agent = c.f.resources.get_agent(&agent.id).await.unwrap().unwrap();
        let control =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            writes.lock().unwrap().len(),
            usize::from(!steer),
            "control: {:?}; operation: {:?}",
            control.work,
            current_agent.conversation.operation
        );
        let logs = c.f.resources.agent_log(&agent.id, 0, 100).await.unwrap();
        assert!(!logs
            .iter()
            .any(|entry| matches!(entry.kind, AgentLogKind::ApprovalResolved { .. })));
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_refuses_unsupported_current_runtime_without_dispatch() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, true, "unsupported-control").await;
    c.f.client.database(&c.f.database).collection::<Document>("agents").update_one(
        doc! {"_id":held.command.receipt.envelope.agent_id.as_str()},
        doc! {"$set":{"record.capabilities":record_bson(&RuntimeCapabilities::new([RuntimeCapability::Conversation])).unwrap()}},
    ).await.unwrap();
    let (execution, writes) = worker::execution(&c, None, false);
    worker::run(&c, &held, execution).await;
    assert!(writes.lock().unwrap().is_empty());
    assert!(matches!(
        worker::service(&c).authorize(&held.command).await,
        Err(ChannelControlError::CapabilityUnsupported)
    ));
    c.f.close().await;
}

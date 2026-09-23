use super::*;
use crate::sandbox::store::{validate_agent_conversation_admission, AgentConversationMutation};

async fn publishing_parent(store: &InMemorySandboxStore, agent: &Agent, rotation: bool) -> Sandbox {
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    let stamp = crate::now_rfc3339();
    let runtime = crate::sandbox::lifecycle_test_support::sandbox()
        .runtime
        .unwrap();
    parent.lifecycle.checkpoints.latest = Some(crate::sandbox::lifecycle_test_support::checkpoint(
        1, &stamp,
    ));
    if rotation {
        parent.runtime = Some(runtime);
        let identity =
            SandboxRotationIdentity::capture(&parent, std::slice::from_ref(agent)).unwrap();
        parent
            .begin_image_rotation("image-v2".into(), identity, stamp.clone())
            .unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation {
            phase,
            checkpoint_epoch,
            ..
        }) = parent.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = SandboxImageRotationPhase::PublishActive;
        *checkpoint_epoch = Some(1);
        parent.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
    } else {
        parent.status = SandboxStatus::Paused;
        parent.runtime = None;
        parent
            .begin_revive(SandboxReviveCause::AgentMessage, stamp.clone())
            .unwrap();
        let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
            parent.lifecycle.operation.as_mut()
        else {
            unreachable!()
        };
        *phase = SandboxRevivePhase::PublishActive;
        parent.runtime = Some(runtime);
    }
    parent
        .work
        .claim(
            "lifecycle-worker".into(),
            &stamp,
            crate::rfc3339_at(SystemTime::now() + Duration::from_secs(30)),
        )
        .unwrap();
    parent.validate().unwrap();
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    parent
}

#[tokio::test]
async fn claimed_message_waits_for_published_active_before_fencing_or_starting_a_turn() {
    for rotation in [false, true] {
        let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        grant(&store, ShareMode::OrgRoles).await;
        let command = claim(&store, &agent, "admin").await;
        let mut parent = publishing_parent(&store, &agent, rotation).await;
        let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
        let authorization = access
            .authorize_command_execution(
                &context("admin"),
                "selected-credential",
                &command.receipt.envelope,
            )
            .await
            .unwrap();
        let (runtime, sends) = runtime(store.clone(), None, 0);
        let operations = AgentOperationService::new(store.clone(), runtime);
        assert!(matches!(
            operations
                .execute_resource_claim(&authorization, &command)
                .await,
            Err(AgentOperationError::StatusConflict { .. })
        ));
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command.clone())
        );
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), history);

        let revision = parent.revision;
        let lease = parent.work.lease.clone().unwrap();
        parent
            .publish_lifecycle_active(
                crate::rfc3339_at(SystemTime::now() + Duration::from_secs(300)),
                &lease.holder,
                lease.fence,
                crate::now_rfc3339(),
            )
            .unwrap();
        store.save_sandbox(revision, parent).await.unwrap();
        operations
            .execute_resource_claim(&authorization, &command)
            .await
            .unwrap();
        let retained = store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.receipt, command.receipt);
        assert!(retained.work.dispatch.is_some());
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn transaction_refuses_claimed_input_before_active_publication() {
    for rotation in [false, true] {
        let (store, _, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let command = claim(&store, &agent, "owner").await;
        let parent = publishing_parent(&store, &agent, rotation).await;
        let mutation = AgentConversationMutation {
            command_claim: Some(command.work.lease.unwrap().claim),
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&parent)),
            expected_agent_revision: agent.revision,
            expected_last_seq: agent.conversation.last_seq,
            runtime_generation: agent.conversation.runtime_generation.unwrap(),
            agent: agent.clone(),
            appended_at: crate::now_rfc3339(),
            agent_logs: vec![AgentLogKind::OwnerMessage {
                content: "retained input".into(),
            }],
        };
        assert_eq!(
            validate_agent_conversation_admission(&agent, Some(&parent), &mutation),
            Err(crate::sandbox::store::SandboxStoreError::Conflict)
        );
    }
}

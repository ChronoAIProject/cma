use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::AgentLogStore;
use layer2_core::sandbox::*;
use std::sync::atomic::{AtomicUsize, Ordering};
mod admission;
mod association;
mod fixtures;
mod guards;
mod readiness;
mod reclaim;
mod reuse;
mod unknown_commit;
mod worker;
use fixtures::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_first_command_replays_exact_binding_and_dispatches_one_shared_turn() {
    let accepted = Accepted::new().await;
    let runtime = Runtime::new(false, false);
    let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
    let replay = accepted
        .f
        .resources
        .admit_agent_with_trigger_initiator(
            accepted.request.clone(),
            accepted.launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(replay.creation().agent.id, accepted.agent.id);
    assert_eq!(
        replay.creation().agent.conversation.first_message,
        accepted.agent.conversation.first_message
    );
    assert!(matches!(
        accepted.command.receipt.envelope.actor,
        AgentActor::Trigger { .. }
    ));
    assert_eq!(accepted.command.receipt.agent_order, 1);
    assert!(matches!(
        service
            .dispatch_queued_first_message(
                &accepted.agent.id,
                AgentWorkAuthority::new("startup".into(), 1).unwrap()
            )
            .await
            .unwrap(),
        FirstMessageDispatchOutcome::CommonCommand(_)
    ));
    let claimed = accepted.claim().await;
    service.execute_trigger_claim(&claimed).await.unwrap();
    assert!(service.execute_trigger_claim(&claimed).await.is_err());
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    let agent = accepted.current_agent().await;
    let first = agent.conversation.first_message.as_ref().unwrap();
    let operation = agent.conversation.operation.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(first.fence_sequence, operation.sequence);
    assert_eq!(first.turn_sequence, Some(operation.turn_sequence));
    assert_eq!(first.origin_key, accepted.agent.creation_key);
    let log = accepted
        .f
        .resources
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        log.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert_eq!(
        log.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        accepted.current_command().await.receipt,
        accepted.command.receipt
    );
    DurableAgentUpdateRecorder::new(accepted.f.resources.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        accepted.current_command().await.status,
        AgentResponseStatus::Completed
    );
    assert_eq!(
        accepted
            .current_agent()
            .await
            .conversation
            .first_message
            .unwrap()
            .state,
        FirstMessageDelivery::Completed
    );
    accepted.f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_prewrite_is_terminal_and_ambiguous_write_never_uses_legacy_recovery() {
    for prewrite in [true, false] {
        let accepted = Accepted::new().await;
        let runtime = Runtime::new(prewrite, !prewrite);
        let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
        let claimed = accepted.claim().await;
        assert!(service.execute_trigger_claim(&claimed).await.is_err());
        let first = accepted
            .current_agent()
            .await
            .conversation
            .first_message
            .unwrap();
        let command = accepted.current_command().await;
        assert_eq!(command.receipt, accepted.command.receipt);
        assert_eq!(
            first.state,
            if prewrite {
                FirstMessageDelivery::Failed
            } else {
                FirstMessageDelivery::RecoveryRequired
            }
        );
        assert_eq!(
            command.status,
            if prewrite {
                AgentResponseStatus::Failed
            } else {
                AgentResponseStatus::WaitingAction
            }
        );
        let caller = Caller::new(
            accepted.agent.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        assert!(service
            .authorize_first_message_recovery(
                &caller,
                &accepted.agent.id,
                &first.payload_sha256,
                true
            )
            .await
            .is_err());
        assert!(service.execute_trigger_claim(&claimed).await.is_err());
        assert_eq!(runtime.sends.load(Ordering::SeqCst), usize::from(!prewrite));
        assert_eq!(
            accepted.current_command().await.receipt,
            accepted.command.receipt
        );
        let mut direct = first.clone();
        assert!(direct.retry_after_pre_write_failure().is_err());
        assert!(direct
            .authorize_recovery(&first.payload_sha256, true, layer2_core::now_rfc3339())
            .is_err());
        assert_eq!(direct, first);
        if !prewrite {
            DurableAgentUpdateRecorder::new(accepted.f.resources.clone())
                .record_update(
                    &accepted.agent.id,
                    1,
                    &mut AgentMessageAssembly::default(),
                    layer3_adapter::ConversationUpdate::TurnEnded(
                        layer3_adapter::PromptResponse::new(
                            layer3_adapter::TurnStopReason::EndTurn,
                        ),
                    ),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
            assert_eq!(
                accepted.current_command().await.status,
                AgentResponseStatus::Completed
            );
            assert_eq!(
                accepted
                    .current_agent()
                    .await
                    .conversation
                    .first_message
                    .unwrap()
                    .state,
                FirstMessageDelivery::Completed
            );
            assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
        }
        accepted.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_lost_binding_sidecars_and_launch_pointer_fail_before_writer() {
    for (collection, field) in [
        ("agents", "trigger_first_message_binding"),
        ("trigger_launches", "trigger_input_binding_sha256"),
    ] {
        let accepted = Accepted::new().await;
        let runtime = Runtime::new(false, false);
        let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
        let claimed = accepted.claim().await;
        let id = if collection == "agents" {
            accepted.agent.id.as_str()
        } else {
            accepted.launch.id.as_str()
        };
        accepted
            .f
            .seed
            .client
            .database(&accepted.f.seed.database)
            .collection::<Document>(collection)
            .update_one(doc! {"_id":id}, doc! {"$unset":{field:""}})
            .await
            .unwrap();
        assert!(service.execute_trigger_claim(&claimed).await.is_err());
        assert!(accepted
            .f
            .resources
            .require_legacy_first_message(&accepted.agent)
            .await
            .is_err());
        assert_eq!(runtime.sends.load(Ordering::SeqCst), 0);
        accepted.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_expiry_closes_first_reservation_without_a_synthetic_turn() {
    let accepted = Accepted::new().await;
    assert_eq!(
        layer2_core::seconds_from_rfc3339(&accepted.command.queue_deadline).unwrap()
            - layer2_core::seconds_from_rfc3339(&accepted.command.receipt.admitted_at).unwrap(),
        300
    );
    accepted
        .f
        .resources
        .reject_unclaimed_command(
            &accepted.command.receipt.envelope.command_id,
            accepted.command.revision,
            CommandFailureCode::QueueTimeout,
            accepted.command.queue_deadline.clone(),
        )
        .await
        .unwrap();
    let agent = accepted.current_agent().await;
    let wire: FirstMessageRecord = serde_json::from_value(
        serde_json::to_value(agent.conversation.first_message.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    wire.validate().unwrap();
    assert_eq!(
        agent.conversation.first_message.unwrap().state,
        FirstMessageDelivery::Failed
    );
    assert!(agent.conversation.active_turn.is_none());
    assert!(agent.conversation.operation.is_none());
    assert_eq!(
        accepted.current_command().await.status,
        AgentResponseStatus::Failed
    );
    accepted.f.close().await;
}

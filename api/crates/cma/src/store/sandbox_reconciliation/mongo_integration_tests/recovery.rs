use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_recovers_checkpoint_agents_preserves_history_and_stops_newer_agents() {
    let (client, name, store) = database().await;
    let (sandbox, mut live) = checkpoint_fixture(&store, "sbx_recover").await;
    for kind in [
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 2,
            operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Preserve this owner's work.".into(),
        },
    ] {
        store
            .append_agent_log(&live.id, BEFORE.into(), kind)
            .await
            .unwrap();
    }
    live = store.get_agent(&live.id).await.unwrap().unwrap();
    live.status = AgentStatus::Working;
    live.conversation.active_turn = Some(3);
    live.conversation.operation = Some(
        AgentOperationRecord::new(
            2,
            1,
            3,
            AgentOperationIntent::send_message("Preserve this owner's work."),
            BEFORE.into(),
        )
        .unwrap(),
    );
    live.work
        .schedule(AgentWorkStep::PublishConnection, BEFORE.into());
    live.work
        .claim("old-worker".into(), BEFORE, OBSERVED.into())
        .unwrap();
    store
        .agents
        .update_one(
            doc! { "_id": live.id.as_str() },
            doc! { "$set": agent_replacement_fields(&live).unwrap() },
        )
        .await
        .unwrap();
    let newer = ready_agent(&sandbox, "agt_newer_checkpoint");
    insert_agent(&store, &newer, BEFORE).await;
    let mut terminal = ready_agent(&sandbox, "agt_already_stopped");
    terminal
        .transition(
            AgentStatus::Stopped,
            BEFORE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    insert_agent(&store, &terminal, BEFORE).await;
    let old_logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    let recovered = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert_eq!(recovered.runtime, None);
    assert_eq!(
        recovered.lifecycle.checkpoints,
        sandbox.lifecycle.checkpoints
    );
    assert_eq!(recovered.profile, sandbox.profile);
    assert_eq!(recovered.credential, sandbox.credential);
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(parked.status, AgentStatus::Paused);
    assert_eq!(parked.conversation.runtime_generation, Some(2));
    assert_eq!(parked.conversation.active_turn, None);
    assert_eq!(parked.work.lease, None);
    assert_eq!(parked.work.step, AgentWorkStep::None);
    let operation = parked.conversation.operation.unwrap();
    assert_eq!(operation.state, AgentOperationDelivery::RuntimeLost);
    assert_eq!(operation.write_confirmed_at, None);
    let logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(&logs[..old_logs.len()], old_logs.as_slice());
    assert_eq!(logs.last().unwrap().seq, parked.conversation.last_seq);
    assert!(logs.iter().any(|entry| matches!(
        entry.kind,
        AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 1,
            runtime_generation: 2,
        }
    )));
    let stopped = store.get_agent(&newer.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::RuntimeLost));
    assert_eq!(
        store.get_agent(&terminal.id).await.unwrap(),
        Some(terminal.clone())
    );
    let terminal_row = store
        .agents
        .find_one(doc! { "_id": terminal.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal_row.get_i64("sandbox_recovery_fence").unwrap(), 1);
    let events = store.sandbox_events(&sandbox.id, 0, 100).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events.last().unwrap().seq, 3);
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    assert_eq!(store.agent_log(&live.id, 0, 100).await.unwrap(), logs);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_without_checkpoint_fails_bound_and_unbound_active_sandboxes() {
    let (client, name, store) = database().await;
    for bound in [true, false] {
        let (mut sandbox, agent) = fixture(&store, &format!("sbx_no_checkpoint_{bound}")).await;
        if !bound {
            sandbox.runtime = None;
            store
                .sandboxes
                .update_one(
                    doc! { "_id": sandbox.id.as_str() },
                    doc! { "$set": sandbox_replacement_fields(&sandbox).unwrap() },
                )
                .await
                .unwrap();
        }
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::Failed
        );
        let failed = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(failed.status, SandboxStatus::Failed);
        assert_eq!(failed.failure.unwrap().class, "runtime_lost");
        assert_eq!(failed.runtime, None);
        assert_eq!(failed.work.step, SandboxWorkStep::None);
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap().status,
            AgentStatus::Stopped
        );
        let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
        assert!(matches!(
            events.last().unwrap().kind,
            SandboxEventKind::Failed { .. }
        ));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_resumes_mid_pause_revive_and_rotation_without_replaying_old_authority() {
    let (client, name, store) = database().await;
    for mode in ["pause", "revive", "rotation"] {
        let (mut sandbox, agent) = checkpoint_fixture(&store, &format!("sbx_mid_{mode}")).await;
        let revision = sandbox.revision;
        match mode {
            "pause" => sandbox.begin_pause(BEFORE.into(), None).unwrap(),
            "rotation" => {
                let identity =
                    SandboxRotationIdentity::capture(&sandbox, std::slice::from_ref(&agent))
                        .unwrap();
                sandbox
                    .begin_image_rotation("image-v2".into(), identity, BEFORE.into())
                    .unwrap();
            }
            "revive" => {
                sandbox.status = SandboxStatus::Paused;
                sandbox.runtime = None;
                sandbox.work.schedule(SandboxWorkStep::None, BEFORE.into());
                sandbox
                    .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                    .unwrap();
                sandbox.runtime = Some(RuntimeBinding {
                    boot_id: None,
                    runtime_id: "lost-replacement".into(),
                    runtime_version: "image-v1".into(),
                    provision_generation: Some(2),
                    bound_at: BEFORE.into(),
                });
                sandbox.lifecycle.runtime_generation_high_water = 2;
            }
            _ => unreachable!(),
        }
        store.save_sandbox(revision, sandbox.clone()).await.unwrap();
        let old_authority = sandbox.lifecycle.next_authority;
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::RecoveryScheduled
        );
        let next = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(next.runtime, None);
        assert!(next.lifecycle.next_authority > old_authority);
        match next.lifecycle.operation.unwrap() {
            SandboxLifecycleOperation::Pause {
                phase,
                checkpoint_epoch,
                ..
            } => {
                assert_eq!(phase, SandboxPausePhase::PublishPaused);
                assert_eq!(checkpoint_epoch, Some(1));
            }
            SandboxLifecycleOperation::ImageRotation {
                phase,
                checkpoint_epoch,
                target_runtime_version,
                ..
            } => {
                assert_eq!(phase, SandboxImageRotationPhase::CheckpointVerification);
                assert_eq!(checkpoint_epoch, Some(1));
                assert_eq!(target_runtime_version, "image-v2");
            }
            SandboxLifecycleOperation::Revive { cause, .. } => {
                assert_eq!(cause, SandboxReviveCause::RuntimeLost)
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}

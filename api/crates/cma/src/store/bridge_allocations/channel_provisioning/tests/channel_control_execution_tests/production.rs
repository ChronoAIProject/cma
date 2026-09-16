//! Exercise the constructor called by ControlPlaneComposition with real gates.
use super::*;
use crate::config::{bridge_flags::BridgeFlag, BridgeFeatureFlags};
use layer3_adapter::ConversationInput;
use std::sync::Mutex;

fn execution(
    c: &ControlFixture,
    enabled: &[BridgeFlag],
    ambiguous: bool,
) -> (
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<Mutex<Vec<ConversationInput>>>,
) {
    let flags = BridgeFeatureFlags::from_lookup(&mut |key| {
        Ok(enabled
            .iter()
            .any(|flag| flag.key() == key)
            .then(|| "true".into()))
    })
    .unwrap();
    // Use the production Bridge gate factory. Live source authority remains
    // the fixture's independent verifier, so no real identity provider is used.
    let mode = crate::auth::AuthMode::Trusted {
        subject: "owned-production-control-fixture".into(),
    };
    let (definitions, credentials) = crate::bridge::service(
        &mode,
        c.f.bridges.clone(),
        crate::auth::AuthState::in_memory(mode.clone(), None),
        c.f.resources.clone(),
        c.f.bridges.credentials(),
        None,
        flags,
    )
    .unwrap();
    assert!(credentials.is_none());
    let (dependencies, writes) = worker::dependencies(c, None, 1, ambiguous);
    let execution = crate::commands::ResourceCommandExecution::production(
        dependencies,
        c.invokes.clone(),
        c.f.invokes.clone(),
        definitions.connection_validator(),
    );
    assert!(execution.channel_controls.is_some());
    (execution, writes)
}

async fn command(c: &ControlFixture, held: &AdmittedChannelControl) -> AdmittedAgentCommand {
    c.f.resources
        .get_command(&held.sidecar.command_id)
        .await
        .unwrap()
        .unwrap()
}

async fn unchanged_source(c: &ControlFixture, held: &AdmittedChannelControl) {
    let agent =
        c.f.resources
            .get_agent(&held.command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    let intent = &held.sidecar.intent;
    assert_eq!(agent.conversation.active_turn, held.command.target_turn);
    assert_eq!(
        agent.conversation.runtime_generation,
        Some(intent.runtime_generation)
    );
    assert_eq!(
        agent.conversation.conversation_id.as_ref(),
        Some(&intent.runtime_conversation_id)
    );
    assert_eq!(
        agent.created_at,
        intent.reference.binding().agent_created_at
    );
    assert_eq!(
        agent.creation_fingerprint,
        intent.reference.binding().creation_fingerprint
    );
    assert_eq!(c.f.items().await.len(), 1, "no automatic control reply");
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_production_gates_hold_release_by_execution_and_exact_platform() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "production-release-gates").await;
    for enabled in [
        vec![],
        vec![BridgeFlag::Execution],
        vec![BridgeFlag::Telegram],
        vec![BridgeFlag::Execution, BridgeFlag::TelegramNew],
    ] {
        let (execution, writes) = execution(&c, &enabled, false);
        let service = execution.channel_controls.as_ref().unwrap();
        assert!(service.authorize(&held.command).await.is_ok());
        assert_eq!(
            service.release_held(&held.command).await,
            Err(ChannelControlError::Unavailable)
        );
        worker::run(&c, &held, execution).await;
        assert_eq!(command(&c, &held).await, held.command);
        assert!(writes.lock().unwrap().is_empty());
        unchanged_source(&c, &held).await;
    }
    let (enabled, writes) = execution(&c, &[BridgeFlag::Execution, BridgeFlag::Telegram], false);
    worker::run(&c, &held, enabled).await;
    assert_eq!(writes.lock().unwrap().len(), 1);
    assert!(matches!(
        &writes.lock().unwrap()[0],
        ConversationInput::Cancel
    ));
    assert_eq!(command(&c, &held).await.receipt, held.command.receipt);
    unchanged_source(&c, &held).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_production_disabled_gates_drain_released_and_expired_claims() {
    for claimed in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, claimed, "production-retained-drain").await;
        let (enabled, _) = execution(&c, &[BridgeFlag::Execution, BridgeFlag::Telegram], false);
        enabled
            .channel_controls
            .as_ref()
            .unwrap()
            .release_held(&held.command)
            .await
            .unwrap();
        let (disabled, writes) = execution(&c, &[], false);
        if claimed {
            let now = std::time::SystemTime::now();
            let original_claim =
                c.f.resources
                    .claim_next_command(CommandClaimRequest {
                        agent_id: held.command.receipt.envelope.agent_id.clone(),
                        holder: "departed-production-control-worker".into(),
                        now: layer2_core::rfc3339_at(now),
                        expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(2)),
                    })
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(original_claim.receipt, held.command.receipt);
            worker::run(&c, &held, disabled.clone()).await;
            assert_eq!(command(&c, &held).await, original_claim);
            assert!(writes.lock().unwrap().is_empty());
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        worker::run(&c, &held, disabled.clone()).await;
        worker::run(&c, &held, disabled).await;
        let after = command(&c, &held).await;
        assert_eq!(after.receipt, held.command.receipt);
        assert_eq!(after.target_turn, held.command.target_turn);
        assert_eq!(
            after.work.observation.as_ref().unwrap().delivery,
            AgentOperationDelivery::WriteConfirmed
        );
        assert_eq!(writes.lock().unwrap().len(), 1);
        assert!(matches!(
            (&writes.lock().unwrap()[0], claimed),
            (ConversationInput::Cancel, false) | (ConversationInput::Prompt(_), true)
        ));
        unchanged_source(&c, &held).await;
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_production_disabled_gates_preserve_unknown_dispatch_on_restart()
{
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "production-unknown-drain").await;
    let (enabled, _) = execution(&c, &[BridgeFlag::Execution, BridgeFlag::Telegram], false);
    enabled
        .channel_controls
        .as_ref()
        .unwrap()
        .release_held(&held.command)
        .await
        .unwrap();
    let (disabled, writes) = execution(&c, &[], true);
    worker::run(&c, &held, disabled).await;
    let unknown = command(&c, &held).await;
    assert_eq!(
        unknown.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::RecoveryRequired
    );
    assert!(!unknown.status.is_terminal());
    assert_eq!(writes.lock().unwrap().len(), 1);
    let (restarted, restart_writes) = execution(&c, &[], false);
    worker::run(&c, &held, restarted.clone()).await;
    worker::run(&c, &held, restarted).await;
    let after = command(&c, &held).await;
    assert_eq!(after.receipt, unknown.receipt);
    assert_eq!(after.work.dispatch, unknown.work.dispatch);
    assert_eq!(
        after.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::RecoveryRequired
    );
    assert!(!after.status.is_terminal());
    assert!(restart_writes.lock().unwrap().is_empty());
    unchanged_source(&c, &held).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_production_disabled_gates_still_require_current_authority() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "production-revoked-drain").await;
    let (enabled, _) = execution(&c, &[BridgeFlag::Execution, BridgeFlag::Telegram], false);
    enabled
        .channel_controls
        .as_ref()
        .unwrap()
        .release_held(&held.command)
        .await
        .unwrap();
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    c.f.invokes
        .revoke(
            &caller(),
            None,
            RevokeBridgeInvokeGrant {
                grant_id: c.f.grant.id.clone(),
                expected_bridge_revision: bridge.revision,
                expected_revision: 1,
                idempotency_key: "production-drain-authority-revoked".into(),
            },
        )
        .await
        .unwrap();
    let (disabled, writes) = execution(&c, &[], false);
    assert!(matches!(
        disabled
            .channel_controls
            .as_ref()
            .unwrap()
            .authorize(&command(&c, &held).await)
            .await,
        Err(ChannelControlError::AuthorityUnavailable)
    ));
    worker::run(&c, &held, disabled).await;
    assert!(command(&c, &held).await.work.dispatch.is_none());
    assert!(writes.lock().unwrap().is_empty());
    unchanged_source(&c, &held).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_production_disabled_gates_settle_only_proved_held_denial() {
    for completed in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, completed, "production-disabled-held-denial").await;
        if completed {
            c.f.finish().await;
            c.f.resources
                .reconcile_command(&c.f.command.receipt.envelope.command_id)
                .await
                .unwrap();
        } else {
            denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
        }
        let before =
            c.f.resources
                .get_agent(&held.command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
        let (disabled, writes) = execution(&c, &[], false);
        worker::run(&c, &held, disabled).await;
        denial::assert_refused(
            &c,
            &held,
            if completed {
                CommandFailureCode::TargetTurnChanged
            } else {
                CommandFailureCode::AuthorityRevoked
            },
        )
        .await;
        assert!(writes.lock().unwrap().is_empty());
        assert_eq!(
            c.f.resources.get_agent(&before.id).await.unwrap().unwrap(),
            before
        );
        c.f.close().await;
    }
}

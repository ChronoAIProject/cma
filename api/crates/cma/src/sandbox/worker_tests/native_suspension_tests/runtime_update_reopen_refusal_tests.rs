use super::*;
use layer2_core::sandbox::store::AgentConversationStore;
use layer2_core::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleAction, SandboxLifecycleResult,
};

#[path = "runtime_update_reopen_relay_tests.rs"]
mod relay;
pub(crate) use relay::exercise_runtime_update_reopen_http;

const TARGET: &str = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn staging_runtime_update_parking_is_exact_and_retireable() {
    for selector_matches in [false, true] {
        let (mut f, parent, agent, mut now) = at_reopen(false, 0).await;
        f.park_runtime_update_sandbox = Some(if selector_matches {
            parent.id.clone()
        } else {
            SandboxId("sbx_00000000000000000000000000000000".into())
        });
        now += Duration::from_secs(6);
        f.step(&parent.id, now, TARGET).await;
        let after = f.harness.stored(&parent.id).await;
        assert_eq!(after.lifecycle.home, parent.lifecycle.home);
        assert_eq!(after.runtime, parent.runtime);
        if !selector_matches {
            assert!(after.runtime_update_request().unwrap().failure.is_none());
            assert!(matches!(
                after.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::QueuedMessageDelivery,
                    ..
                })
            ));
            continue;
        }

        assert_eq!(after.lifecycle.operation, parent.lifecycle.operation);
        assert_eq!(after.work.step, SandboxWorkStep::None);
        assert!(after.work.lease.is_none());
        assert_eq!(
            f.harness.store.get_agent(&agent.id).await.unwrap(),
            Some(agent)
        );
        let result = SandboxLifecycleResult::observe(&after, None, now, true).unwrap();
        assert_eq!(result.code, Some("runtime_update_parked"));
        assert!(!result.continuing);
        assert!(result.actions.contains(&SandboxLifecycleAction::Retire));

        let retiring = f
            .harness
            .service
            .delete(&caller("owner", [Capability::DeleteOwnSandbox]), &after.id)
            .await
            .unwrap();
        assert_eq!(retiring.status, SandboxStatus::Retiring);
        let retired = f
            .drive(&after.id, &mut now, SandboxStatus::Retired, TARGET)
            .await;
        assert!(retired.lifecycle.home.unwrap().released);
    }
}

#[tokio::test]
async fn staging_runtime_update_parking_leaves_ordinary_revive_unchanged() {
    let (mut f, parent, _, now) = retained_reopen_tests::restored_reopen().await;
    f.park_runtime_update_sandbox = Some(parent.id.clone());
    f.step(&parent.id, now + Duration::from_secs(6), &runtime_image())
        .await;
    let after = f.harness.stored(&parent.id).await;
    assert!(after.runtime_update_request().is_none());
    assert!(matches!(
        after.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::QueuedMessageDelivery,
            ..
        })
    ));
}

#[tokio::test]
async fn runtime_update_reopen_output_refusal_parks_only_the_enabled_current_rotation() {
    for (enabled, current, attempts, cursors, parked) in [
        (false, true, 0, vec![2], false),
        (true, false, 0, vec![2], false),
        (true, true, 0, vec![2], true),
        (true, true, 8_700, vec![2], true),
        (true, true, 0, vec![1, 3], true),
    ] {
        let (f, parent, agent, now) = at_reopen(enabled, attempts).await;
        *f.backend.reopen_output.cursors.lock().unwrap() = cursors.clone();
        let released = if current {
            TARGET.to_owned()
        } else {
            runtime_image()
        };
        f.step(&parent.id, now + Duration::from_secs(6), &released)
            .await;
        let mut after = f.harness.stored(&parent.id).await;
        if cursors.len() > 1 {
            assert_eq!(after.work.step, SandboxWorkStep::RunLifecycleOperation);
            assert!(after.runtime_update_request().unwrap().failure.is_none());
            f.step(&parent.id, now + Duration::from_secs(12), &released)
                .await;
            after = f.harness.stored(&parent.id).await;
        }
        assert_eq!(after.runtime, parent.runtime);
        assert_eq!(after.lifecycle.home, parent.lifecycle.home);
        assert_eq!(after.lifecycle.operation, parent.lifecycle.operation);
        assert_eq!(after.work.step == SandboxWorkStep::None, parked);
        assert!(after.work.lease.is_none());
        assert_eq!(
            after
                .runtime_update_request()
                .unwrap()
                .failure
                .as_ref()
                .map(|failure| failure.class.as_str()),
            parked.then_some("terminal_validation"),
        );
        assert_eq!(
            f.harness.store.get_agent(&agent.id).await.unwrap().unwrap(),
            agent
        );
        let acked = f.backend.reopen_output.acknowledged.lock().unwrap().clone();
        let expected_acks = if cursors.len() > 1 {
            vec![1, 1]
        } else {
            Vec::new()
        };
        assert_eq!(acked, expected_acks);
        let checkpoint = f
            .harness
            .store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap();
        assert_eq!(
            checkpoint.and_then(|checkpoint| checkpoint.output.map(|output| output.cursor)),
            acked.last().copied()
        );
    }
}

#[tokio::test]
async fn runtime_update_reopen_output_refusal_preserves_exact_replay_and_publication() {
    let (f, parent, agent, now) = at_reopen(true, 0).await;
    *f.backend.reopen_output.cursors.lock().unwrap() = vec![1, 1, 2, 1];
    f.step(&parent.id, now + Duration::from_secs(6), TARGET)
        .await;
    let after = f.harness.stored(&parent.id).await;
    assert!(matches!(
        after.lifecycle.operation,
        Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::QueuedMessageDelivery,
            ..
        })
    ));
    assert_eq!(
        *f.backend.reopen_output.acknowledged.lock().unwrap(),
        vec![1, 1, 2, 1]
    );
    let ready = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(
        ready.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        ready.conversation.runtime_generation,
        parent.runtime.unwrap().provision_generation
    );
    assert_eq!(
        f.harness
            .store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap()
            .cursor,
        2
    );
}

#[tokio::test]
async fn runtime_update_reopen_output_refusal_leaves_ordinary_revive_retryable() {
    let (mut f, parent, agent, now) = retained_reopen_tests::restored_reopen().await;
    f.reopen_output_refusal = true;
    *f.backend.reopen_output.cursors.lock().unwrap() = vec![2];
    f.step(&parent.id, now + Duration::from_secs(6), &runtime_image())
        .await;
    let after = f.harness.stored(&parent.id).await;
    assert_eq!(after.lifecycle.operation, parent.lifecycle.operation);
    assert_eq!(after.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert!(after.runtime_update_request().is_none());
    assert!(f
        .backend
        .reopen_output
        .acknowledged
        .lock()
        .unwrap()
        .is_empty());
    assert!(f
        .harness
        .store
        .agent_message_checkpoint(&agent.id)
        .await
        .unwrap()
        .is_none());
}

async fn at_reopen(
    enabled: bool,
    attempts: u32,
) -> (Fixture, Sandbox, layer2_core::sandbox::Agent, SystemTime) {
    let (mut f, active, mut now) = active().await;
    f.reopen_output_refusal = enabled;
    let agent = lifecycle_recovery_tests::prepare_agent_with_conversation(
        &f.harness,
        &active,
        now,
        1,
        Some(uuid::Uuid::new_v4().to_string()),
        None,
    )
    .await;
    f.harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    update(&f, &active, TARGET, now).await;
    for _ in 0..12 {
        let mut parent = f.harness.stored(&active.id).await;
        if matches!(
            parent.lifecycle.operation,
            Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::ConversationReopen,
                ..
            })
        ) {
            if attempts != 0 {
                parent.work.attempts = attempts;
                parent.revision += 1;
                f.harness
                    .store
                    .save_sandbox(parent.revision - 1, parent.clone())
                    .await
                    .unwrap();
            }
            let agent = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
            assert_eq!(agent.status, AgentStatus::Paused);
            return (f, parent, agent, now);
        }
        now += Duration::from_secs(6);
        f.step(&active.id, now, TARGET).await;
    }
    panic!("runtime update fixture did not reach conversation reopen");
}

use super::{reuse_command_fixture::ReuseCommandFixture, *};
use layer2_core::sandbox::{agent_io::*, store::*, *};
use std::time::SystemTime;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn accepted_reused_child_command_revives_its_original_target_after_binding_reset() {
    for shared_workspace in [false, true] {
        let mut f = ReuseCommandFixture::new(shared_workspace, false).await;
        let queued = f.queued("reuse-revival-before-reset").await;
        let original = f.record.clone();
        let original_agent =
            f.f.original
                .resources
                .get_agent(&original.placement.target().agent_id)
                .await
                .unwrap()
                .unwrap();
        let next = f.f.reset(&original, "67890").await.unwrap();
        assert_ne!(next.placement.target().agent_id, original_agent.id);
        f.record =
            f.f.original
                .allocations
                .generation(&original.identity)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(f.record.state, AllocationState::Retired);
        assert_eq!(f.record.placement, original.placement);
        let paused = f.pause().await;
        let permit = f.permit().await;
        let execution = queued
            .channel
            .authorize_execution(&queued.invokes, &queued.command)
            .await
            .unwrap();
        queued
            .store
            .schedule_reused_channel_command_revival(
                &f.f.original.allocations,
                &permit,
                &execution.fence(),
                &queued.command,
            )
            .await
            .unwrap();
        for _ in 0..12 {
            if f.sandbox().await.status == SandboxStatus::Active {
                break;
            }
            f.step().await;
        }
        let active = f.sandbox().await;
        assert_eq!(active.status, SandboxStatus::Active);
        assert_eq!(active.id, paused.id);
        assert_eq!(active.origin, paused.origin);
        assert!(f.f.current(&next).await == next);
        assert!(f
            .f
            .original
            .resources
            .get_agent(&next.placement.target().agent_id)
            .await
            .unwrap()
            .is_none());
        let restored =
            f.f.original
                .resources
                .get_agent(&original_agent.id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            restored.conversation.conversation_id,
            original_agent.conversation.conversation_id
        );
        let stamp = SystemTime::now();
        let claimed =
            f.f.original
                .resources
                .claim_next_command(CommandClaimRequest {
                    agent_id: original_agent.id,
                    holder: "retained-reused-command-worker".into(),
                    now: layer2_core::rfc3339_at(stamp),
                    expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
                })
                .await
                .unwrap()
                .unwrap();
        assert_eq!(claimed.receipt, queued.command.receipt);
        let execution = queued
            .channel
            .authorize_execution(&queued.invokes, &claimed)
            .await
            .unwrap();
        AgentOperationService::new(f.f.original.resources.clone(), f.registry.clone())
            .execute_channel_claim(&execution, &claimed)
            .await
            .unwrap();
        f.reply_mid_turn(&queued).await;
        f.close().await;
    }
}

use super::execution_tests::execution;
use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn missing_channel_target_bridge_or_grant_refuses_hold_immediately_without_dispatch() {
    for reason in [
        ChannelHoldRefusalReason::TargetMissing,
        ChannelHoldRefusalReason::BridgeMissing,
        ChannelHoldRefusalReason::GrantMissing,
    ] {
        let c = ChannelFixture::new().await;
        let held = c
            .commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap();
        let (execution, sends) = execution(&c, false);
        let channel = execution.channel.as_ref().unwrap();
        // The caller cannot manufacture absence from a provider outage or an
        // unavailable deployment flag: each retained source still exists.
        assert!(channel.refuse_execution_hold(&held, reason).await.is_err());
        let (collection, id) = match reason {
            ChannelHoldRefusalReason::TargetMissing => {
                (&c.f.resources.agents, c.f.target.agent_id.as_str())
            }
            ChannelHoldRefusalReason::BridgeMissing => {
                (&c.f.bridges.definitions, c.f.bridge.id.as_str())
            }
            ChannelHoldRefusalReason::GrantMissing => (&c.f.store.grants, c.grant.id.as_str()),
            _ => unreachable!(),
        };
        let removed = collection
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        collection.delete_one(doc! {"_id": id}).await.unwrap();
        assert!(!held.queue_expired(&layer2_core::now_rfc3339()));
        AgentCommandWorker::new(
            c.f.resources.clone(),
            execution,
            "missing-source-worker".into(),
        )
        .unwrap()
        .run_one(&held.receipt.envelope.command_id)
        .await
        .unwrap();
        let current =
            c.f.resources
                .get_command(&held.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(current.status, AgentResponseStatus::Failed);
        assert_eq!(
            current.work.failure.as_ref().unwrap().code,
            match reason {
                ChannelHoldRefusalReason::TargetMissing => CommandFailureCode::AgentUnavailable,
                _ => CommandFailureCode::AuthorityRevoked,
            }
        );
        assert_eq!(current.receipt, held.receipt);
        assert!(current.work.dispatch.is_none() && current.work.lease.is_none());
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        // Even if a fixture restores the exact source later, this accepted
        // command keeps its truthful terminal receipt and cannot be replayed.
        collection.insert_one(removed).await.unwrap();
        assert_eq!(
            c.f.resources
                .get_command(&held.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(current)
        );
        c.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn released_channel_command_with_missing_target_is_rejected_before_runtime_io() {
    let c = ChannelFixture::new().await;
    let held = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (execution, sends) = execution(&c, false);
    execution.release_held(&held).await;
    let released =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(released.status, AgentResponseStatus::Queued);
    c.f.resources
        .agents
        .delete_one(doc! {"_id": c.f.target.agent_id.as_str()})
        .await
        .unwrap();
    assert_eq!(
        execution.check_queued(&released).await,
        CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
    );
    AgentCommandWorker::new(
        c.f.resources.clone(),
        execution,
        "released-missing-worker".into(),
    )
    .unwrap()
    .run_one(&held.receipt.envelope.command_id)
    .await
    .unwrap();
    let current =
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.status, AgentResponseStatus::Failed);
    assert_eq!(
        current.work.failure.as_ref().unwrap().code,
        CommandFailureCode::AgentUnavailable
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    c.close().await;
}

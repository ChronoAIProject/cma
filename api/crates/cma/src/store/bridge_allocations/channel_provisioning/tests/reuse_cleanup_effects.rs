use super::*;
use layer2_core::bridge::outbox::*;

struct UnknownReply;
#[async_trait::async_trait]
impl RelayDeliveryPort for UnknownReply {
    async fn deliver(&self, _: RelayDeliveryRequest) -> DeliveryOutcome {
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_keeps_pending_and_unknown_effects_and_original_observations() {
    for unknown_before_request in [false, true] {
        let mut f = CleanupFixture::new(true).await;
        let queued = f.f.queued("cleanup-original-command").await;
        let now = SystemTime::now();
        let claimed =
            f.f.f
                .original
                .resources
                .claim_next_command(CommandClaimRequest {
                    agent_id: queued.command.receipt.envelope.agent_id.clone(),
                    holder: "cleanup-original-dispatch".into(),
                    now: layer2_core::rfc3339_at(now),
                    expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
                })
                .await
                .unwrap()
                .unwrap();
        let authority = queued
            .channel
            .authorize_execution(&queued.invokes, &claimed)
            .await
            .unwrap();
        AgentOperationService::new(f.f.f.original.resources.clone(), f.f.registry.clone())
            .execute_channel_claim(&authority, &claimed)
            .await
            .unwrap();
        let replies = f.f.reply_mid_turn(&queued).await;
        f.reset().await;
        if unknown_before_request {
            assert!(replies
                .deliver_one("cleanup-unknown-first", &UnknownReply)
                .await
                .unwrap());
        }
        let command_id = queued.command.receipt.envelope.command_id.as_str();
        let before_command = f.row("agent_commands", command_id).await;
        let before_reply = f
            .db()
            .collection::<Document>("bridge_outbox_items")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        let state = f.request_cleanup().await.unwrap();
        assert!(state.holds.contains(&ChannelReuseCleanupHold::AgentBusy));
        assert!(state
            .holds
            .contains(&ChannelReuseCleanupHold::PendingCommand));
        assert!(state.holds.contains(&if unknown_before_request {
            ChannelReuseCleanupHold::UncertainReply
        } else {
            ChannelReuseCleanupHold::PendingReply
        }));
        assert_eq!(f.row("agent_commands", command_id).await, before_command);
        assert_eq!(
            f.db()
                .collection::<Document>("bridge_outbox_items")
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap(),
            before_reply
        );
        if !unknown_before_request {
            assert!(replies
                .deliver_one("cleanup-unknown-later", &UnknownReply)
                .await
                .unwrap());
            // Replaying an immutable request does not pretend to refresh holds.
            assert!(f.request_cleanup().await.unwrap() == state);
        }
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_holds_for_historical_extra_children_and_scan_overflow() {
    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    let target = f.original.placement.target();
    let base =
        f.f.f
            .original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let rows = f.db().collection::<Document>("agents");
    let mut extra = base.clone();
    extra.id = layer2_core::AgentId("extra-retained-child".into());
    extra.worktree_id = layer2_core::WorktreeId("extra-retained-tree".into());
    extra.creation_key = "retained-child-creation".into();
    extra.creation_fingerprint = "a".repeat(64);
    extra
        .transition(
            AgentStatus::Stopped,
            layer2_core::now_rfc3339(),
            Some(AgentStopReason::Operator),
        )
        .unwrap();
    rows.insert_one(crate::store::agents::agent_document(&extra).unwrap())
        .await
        .unwrap();
    let state = f.request_cleanup().await.unwrap();
    assert!(state
        .holds
        .contains(&ChannelReuseCleanupHold::OtherWorkspaceChildren));
    f.close().await;

    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    let target = f.original.placement.target();
    let base =
        f.f.f
            .original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let mut extra_rows = Vec::new();
    for index in 0..257 {
        let mut extra = base.clone();
        extra.id = layer2_core::AgentId(format!("overflow-child-{index}"));
        extra.worktree_id = layer2_core::WorktreeId(format!("overflow-tree-{index}"));
        extra.creation_key = format!("overflow-creation-{index}");
        extra.creation_fingerprint = "b".repeat(64);
        extra_rows.push(crate::store::agents::agent_document(&extra).unwrap());
    }
    f.db()
        .collection::<Document>("agents")
        .insert_many(extra_rows)
        .await
        .unwrap();
    let state = f.request_cleanup().await.unwrap();
    assert!(state
        .holds
        .contains(&ChannelReuseCleanupHold::InventoryLimit));
    assert!(state
        .holds
        .contains(&ChannelReuseCleanupHold::OtherWorkspaceChildren));
    f.close().await;
}

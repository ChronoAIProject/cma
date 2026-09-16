use super::{reuse_command_fixture::ReuseCommandFixture, *};
use layer2_core::{
    bridge::outbox::*,
    sandbox::{agent_io::*, store::*, *},
};
use mongodb::bson::Document;
use std::{sync::Mutex, time::SystemTime};

struct OriginalReply {
    source: OutboxSource,
    calls: Mutex<usize>,
}
#[async_trait::async_trait]
impl RelayDeliveryPort for OriginalReply {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        assert!(request.item().source == self.source);
        *self.calls.lock().unwrap() += 1;
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "stopped-original-reuse-child".into(),
            platform_message_id: None,
        })
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn queued_original_reply_after_reset_survives_stopped_child_in_both_reuse_modes() {
    for shared_workspace in [false, true] {
        let f = ReuseCommandFixture::new(shared_workspace, false).await;
        let queued = f.queued("reused-child-before-stop").await;
        let resources = &f.f.original.resources;
        let stamp = SystemTime::now();
        let claimed = resources
            .claim_next_command(CommandClaimRequest {
                agent_id: queued.command.receipt.envelope.agent_id.clone(),
                holder: "stop-reply-worker".into(),
                now: layer2_core::rfc3339_at(stamp),
                expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
        let execution = queued
            .channel
            .authorize_execution(&queued.invokes, &claimed)
            .await
            .unwrap();
        AgentOperationService::new(resources.clone(), f.registry.clone())
            .execute_channel_claim(&execution, &claimed)
            .await
            .unwrap();
        let replies = f.reply_mid_turn(&queued).await;
        f.f.reset(&f.record, "67890").await.unwrap();
        let database = f.f.original.client.database(&f.f.original.database);
        let row = database
            .collection::<Document>("bridge_outbox_items")
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        let item: OutboxItem =
            mongodb::bson::deserialize_from_document(row.get_document("record").unwrap().clone())
                .unwrap();
        let mut agent = resources
            .get_agent(&claimed.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        // Only the already stopped child's durable identity is relevant here.
        // Original parent ownership/lifecycle and all reply authority stay live.
        agent
            .transition(
                AgentStatus::Stopped,
                layer2_core::now_rfc3339(),
                Some(AgentStopReason::Owner),
            )
            .unwrap();
        resources.save_agent(revision, agent).await.unwrap();
        let current = resources
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert!(queued
            .channel
            .authorize_execution(&queued.invokes, &current)
            .await
            .is_err());
        let relay = OriginalReply {
            source: item.source,
            calls: Mutex::new(0),
        };
        assert!(replies
            .deliver_one("stopped-reused-original-reply", &relay)
            .await
            .unwrap());
        assert_eq!(*relay.calls.lock().unwrap(), 1);
        assert_eq!(
            resources
                .get_command(&claimed.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap()
                .receipt,
            queued.command.receipt
        );
        f.close().await;
    }
}

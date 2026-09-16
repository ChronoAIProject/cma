use super::*;
use layer2_core::sandbox::store::AgentStore;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_newer_inbound_replies_keep_new_source_and_original_created_context_under_drain_original(
) {
    let f = ReplyFixture::with_newer_inbound().await;
    assert_ne!(f.inbox.pin, f.allocation.pin);
    assert_eq!(f.grant.pin, f.inbox.pin);
    let context = f.context().await;
    let ack = f
        .reply(
            &context,
            "new-generation-ack",
            "I am continuing in the existing context.",
        )
        .await;
    let progress = f
        .reply(
            &context,
            "new-generation-progress",
            "The current input is being processed.",
        )
        .await;
    for result in [ack, progress] {
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["structuredContent"]["status"], "queued");
    }
    let layer2_core::bridge::allocation::AllocationPlacement::NewSandbox { profiles, target } =
        &f.allocation.placement
    else {
        unreachable!()
    };
    let agent = f
        .resources
        .get_agent(&target.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.profile, profiles.agent);
    let rows = f.items().await;
    assert_eq!(rows.len(), 2);
    for row in rows {
        let source = row
            .get_document("record")
            .unwrap()
            .get_document("source")
            .unwrap();
        let pin: BridgeWorkPin =
            mongodb::bson::deserialize_from_document(source.get_document("pin").unwrap().clone())
                .unwrap();
        assert_eq!(pin, f.inbox.pin);
        assert_eq!(source.get_str("allocation_id").unwrap(), f.allocation.id);
        assert_eq!(source.get_str("message_id").unwrap(), f.inbox.message_id);
        let config: BridgeConfig = mongodb::bson::deserialize_from_document(
            source.get_document("config").unwrap().clone(),
        )
        .unwrap();
        let BridgePlacementPolicy::NewSandbox { agent_profile, .. } = config.placement else {
            unreachable!()
        };
        assert_eq!(agent_profile.revision, profiles.agent.revision + 1);
    }
    f.close().await;
}

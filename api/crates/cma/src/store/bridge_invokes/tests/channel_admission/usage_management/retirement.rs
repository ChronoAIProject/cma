use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_org_retire_is_atomic_attributed_and_replay_preserves_original() {
    let c = ChannelFixture::from_fixture(Fixture::organization().await).await;
    let command = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    let allocation_before = allocation_document(&c).await;
    let request = retire(&c, AcceptedWorkPolicy::DrainOriginal);
    fail_events(&c, true).await;
    assert!(mutate(&c, request.clone()).await.is_err());
    assert_eq!(history(&c).await, before);
    assert_eq!(allocation_document(&c).await, allocation_before);
    assert!(c.f.bridges.bridge(&c.f.bridge.id).await.unwrap().unwrap() == c.f.bridge);
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_definition_receipts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    fail_events(&c, false).await;
    let receipt = mutate(&c, request.clone()).await.unwrap();
    assert_eq!(mutate(&c, request).await.unwrap(), receipt);
    let (after, events) = history(&c).await;
    assert_eq!(events.len(), 3);
    assert!(after.active_bindings.is_empty());
    assert_eq!(after.creation, before.0.creation);
    assert_eq!(after.first_used_at, before.0.first_used_at);
    assert_eq!(after.last_used_at, before.0.last_used_at);
    assert_eq!(events[2].before, before.0);
    assert_eq!(events[2].after, after);
    assert_eq!(events[2].actor, c.f.context.actor);
    assert_ne!(events[2].actor, caller().actor);
    assert_eq!(events[2].reason, ChannelBindingChangeReason::Detachment);
    assert_eq!(events[2].detached_at.as_ref(), Some(&events[2].at));
    let closed = allocation_document(&c).await;
    let closure = closed.get_document("channel_usage_closure").unwrap();
    let stored: StoredBridgeReceipt =
        mongodb::bson::deserialize_from_bson(closure.get("management").unwrap().clone()).unwrap();
    assert_eq!(stored.receipt, receipt);
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        command
    );
    assert_eq!(history(&c).await, (after, events));
    let mut replayed = allocation_document(&c).await;
    let mut closed = closed;
    assert_eq!(
        replayed.get_i64("command_admission_fence").unwrap(),
        closed.get_i64("command_admission_fence").unwrap() + 1
    );
    replayed.remove("command_admission_fence");
    closed.remove("command_admission_fence");
    assert_eq!(replayed, closed);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_late_first_command_after_retire_is_only_real_historical_use() {
    let c = ChannelFixture::new().await;
    let before = history(&c).await;
    let receipt = mutate(&c, retire(&c, AcceptedWorkPolicy::DrainOriginal))
        .await
        .unwrap();
    let closed = allocation_document(&c).await;
    assert_eq!(history(&c).await, before);
    let accepted_after = layer2_core::now_rfc3339();
    let command = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (state, events) = history(&c).await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert!(matches!(event.change, ChannelUsageChange::Use { .. }));
    assert_eq!(event.reason, ChannelBindingChangeReason::AcceptedCommand);
    assert_eq!(event.actor, command.receipt.envelope.actor);
    assert!(matches!(event.actor, ResourceActor::Channel { .. }));
    assert!(event.at >= accepted_after && event.at >= receipt.applied_at);
    assert!(event.detached_at.is_none());
    assert!(state.active_bindings.is_empty() && !state.summary().active);
    assert_eq!(state.creation, before.0.creation);
    assert_eq!(state.first_used_at.as_ref(), Some(&event.at));
    assert_eq!(state.last_used_at.as_ref(), Some(&event.at));
    let binding = event.predecessor.as_ref().unwrap();
    assert_eq!(binding.bridge_id, c.f.bridge.id.as_str());
    let current = allocation_document(&c).await;
    assert_eq!(
        current.get("channel_usage_closure"),
        closed.get("channel_usage_closure")
    );
    assert_eq!(
        current
            .get_document("channel_usage_binding")
            .unwrap()
            .get_i64("linked_grants")
            .unwrap(),
        0
    );
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        command
    );
    assert_eq!(history(&c).await, (state, events));
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_display_update_keeps_binding_and_suspension_still_refuses() {
    let c = ChannelFixture::new().await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    let allocation = allocation_document(&c).await;
    let mut config = c.f.bridge.current().unwrap().config.clone();
    config.display_name = "Updated display only".into();
    mutate(
        &c,
        BridgeMutation {
            id: c.f.bridge.id.clone(),
            expected_revision: c.f.bridge.revision,
            idempotency_key: "display".into(),
            change: BridgeChange::Update {
                config: Box::new(config),
                previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(history(&c).await, before);
    assert_eq!(allocation_document(&c).await, allocation);
    c.close().await;
    let c = ChannelFixture::new().await;
    let pending = c.request().await;
    mutate(&c, retire(&c, AcceptedWorkPolicy::SuspendOriginal))
        .await
        .unwrap();
    assert!(c.commands.reserve_channel_command(pending).await.is_err());
    assert!(history(&c).await.1.is_empty());
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_corrupt_closure_or_historical_event_failure_is_atomic() {
    for corrupt in [true, false] {
        let c = ChannelFixture::new().await;
        mutate(&c, retire(&c, AcceptedWorkPolicy::DrainOriginal))
            .await
            .unwrap();
        if corrupt {
            c.f.client
                .database(&c.f.database)
                .collection::<Document>("bridge_allocations")
                .update_one(
                    doc! {"_id":&c.allocation.id},
                    doc! {"$set":{
                    "channel_usage_closure.management.receipt.actor.subject":"someone-else"}},
                )
                .await
                .unwrap();
        } else {
            fail_events(&c, true).await;
        }
        let before = allocation_document(&c).await;
        assert!(c
            .commands
            .reserve_channel_command(c.request().await)
            .await
            .is_err());
        assert_eq!(allocation_document(&c).await, before);
        assert!(history(&c).await.1.is_empty());
        assert_eq!(
            c.f.resources
                .agent_commands
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        if !corrupt {
            fail_events(&c, false).await;
            c.commands
                .reserve_channel_command(c.request().await)
                .await
                .unwrap();
            assert_eq!(history(&c).await.1.len(), 1);
        }
        c.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_retire_preserves_old_identity_after_removal_or_recreation() {
    for recreated in [false, true] {
        let c = ChannelFixture::new().await;
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap();
        let before = history(&c).await;
        let id = c.f.target.agent_id.as_str();
        if recreated {
            c.f.resources
                .agents
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{"record.created_at":"2001-01-01T00:00:00Z"}},
                )
                .await
                .unwrap();
        } else {
            c.f.resources
                .agents
                .delete_one(doc! {"_id":id})
                .await
                .unwrap();
        }
        let target_before =
            c.f.resources
                .agents
                .find_one(doc! {"_id":id})
                .await
                .unwrap();
        mutate(&c, retire(&c, AcceptedWorkPolicy::DrainOriginal))
            .await
            .unwrap();
        assert_eq!(
            c.f.resources
                .agents
                .find_one(doc! {"_id":id})
                .await
                .unwrap(),
            target_before
        );
        let row =
            c.f.resources
                .channel_usage_events
                .find_one(doc! {"agent_id":id})
                .sort(doc! {"revision":-1})
                .await
                .unwrap()
                .unwrap();
        let event = crate::store::agent_channel_usage::read_usage_event(&row).unwrap();
        assert_eq!(event.before, before.0);
        assert_eq!(event.after.creation, before.0.creation);
        assert!(event.after.active_bindings.is_empty());
        assert_eq!(event.actor, c.f.context.actor);
        c.close().await;
    }
}

use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_rebind_late_work_and_retire_keep_original_manager_and_sibling() {
    for first_use in [true, false] {
        let mut c = ChannelFixture::new().await;
        if !first_use {
            c.commands
                .reserve_channel_command(c.request().await)
                .await
                .unwrap();
        }
        let before = history(&c).await;
        if !first_use {
            accept_next_inbox(&mut c, 63).await;
        }
        let sibling = second_agent(&c).await;
        let sibling_before =
            c.f.resources
                .agent_channel_usage_snapshot(&sibling.agent_id)
                .await
                .unwrap();
        let request = rebind(&c, sibling.clone()).await;
        let receipt = mutate(&c, request.clone()).await.unwrap();
        assert_eq!(mutate(&c, request).await.unwrap(), receipt);
        let closed = allocation_document(&c).await;
        let (after, events) = history(&c).await;
        assert!(after.active_bindings.is_empty());
        if first_use {
            assert!(events.is_empty());
        } else {
            assert_eq!(events.len(), before.1.len() + 1);
            assert_eq!(
                events.last().unwrap().reason,
                ChannelBindingChangeReason::Rebind
            );
            assert_eq!(events.last().unwrap().actor, c.f.context.actor);
        }
        // The fixture validator observes the actual target of the pinned old
        // configuration when checking its still-authorized DrainOriginal work.
        select_connection_target(&c, &c.f.target.agent_id).await;
        // An inbox was genuinely accepted before rebind; its first common
        // admission drains against its exact old allocation and grant.
        {
            let count_before = history(&c).await.1.len();
            let command = c
                .commands
                .reserve_channel_command(c.request().await)
                .await
                .unwrap();
            let (state, events) = history(&c).await;
            assert_eq!(events.len(), count_before + 1);
            assert!(state.active_bindings.is_empty());
            assert!(events.last().unwrap().predecessor.is_some());
            assert_eq!(events.last().unwrap().actor, command.receipt.envelope.actor);
            assert_eq!(
                c.commands
                    .reserve_channel_command(c.request().await)
                    .await
                    .unwrap(),
                command
            );
            assert_eq!(history(&c).await, (state, events));
        }
        c.f.bridge = c.f.bridges.bridge(&c.f.bridge.id).await.unwrap().unwrap();
        let before_retire = history(&c).await;
        mutate(&c, retire(&c, AcceptedWorkPolicy::DrainOriginal))
            .await
            .unwrap();
        assert_eq!(history(&c).await, before_retire);
        assert_eq!(
            allocation_document(&c).await.get("channel_usage_closure"),
            closed.get("channel_usage_closure")
        );
        assert_eq!(
            c.f.resources
                .agent_channel_usage_snapshot(&sibling.agent_id)
                .await
                .unwrap(),
            sibling_before
        );
        c.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_explicit_reset_after_historical_first_use_keeps_real_predecessor()
{
    use layer2_core::bridge::{allocation::*, partition::*};
    let mut c = ChannelFixture::new().await;
    let old_target = c.f.target.clone();
    let sibling = second_agent(&c).await;
    mutate(&c, rebind(&c, sibling.clone()).await).await.unwrap();
    select_connection_target(&c, &old_target.agent_id).await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let historical = history(&c).await.1.remove(0);
    let predecessor = historical.predecessor.unwrap();
    c.f.bridge = c.f.bridges.bridge(&c.f.bridge.id).await.unwrap().unwrap();
    c.f.target = sibling;
    select_connection_target(&c, &c.f.target.agent_id).await;
    c.grant =
        c.f.service
            .issue(
                &c.f.context,
                Some("fixture-credential"),
                c.f.issue("new-target-consent"),
            )
            .await
            .unwrap()
            .result;
    accept_next_inbox(&mut c, 71).await;
    let source = NormalizedPartitionSource::from_stored(&c.inbox, &c.f.bridge).unwrap();
    let admission = AllocationAdmission::new(c.f.bridge.clone(), &source)
        .unwrap()
        .after_reset(&c.allocation)
        .unwrap();
    let proof =
        c.f.service
            .authorize(c.f.request(
                &c.grant,
                BridgeInvokeEffect::BindExistingTarget {
                    allocation_id: admission.identity().id().unwrap(),
                },
            ))
            .await
            .unwrap();
    let management =
        c.f.definitions
            .management_lease(
                &c.f.context,
                Some("fixture-credential"),
                &c.f.bridge.id,
                c.f.bridge.revision,
            )
            .await
            .unwrap();
    let store = crate::store::MongoBridgeAllocationStore::open(
        c.f.bridges.clone(),
        Some(c.f.store.clone()),
        &c.f.database,
    )
    .await
    .unwrap();
    c.allocation = store
        .reserve(
            AllocationReservation::reset(
                admission,
                AllocationAuthority::Existing(Box::new(proof)),
                c.allocation.clone(),
                management,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (state, events) = history(&c).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].predecessor.as_ref(), Some(&predecessor));
    assert_eq!(state.active_bindings.len(), 1);
    assert_eq!(
        state.active_bindings[0]
            .reference
            .previous_binding_id
            .as_ref(),
        Some(&predecessor.binding_id)
    );
    assert_eq!(
        state.active_bindings[0].reference.generation,
        predecessor.generation + 1
    );
    let old =
        c.f.resources
            .agent_channel_usage_snapshot(&old_target.agent_id)
            .await
            .unwrap();
    assert!(old.state.active_bindings.is_empty());
    c.close().await;
}

use super::*;
use layer2_core::sandbox::{
    channel_usage::{AgentChannelUsageState, ChannelUsageEvent},
    store::{AgentChannelUsageStore, PageRequest},
};

async fn history(
    resources: &crate::store::MongoSandboxStore,
    agent: &layer2_core::AgentId,
) -> (AgentChannelUsageState, Vec<ChannelUsageEvent>) {
    let (snapshot, page) = resources
        .agent_channel_usage_history(agent, &PageRequest::new(100, None).unwrap())
        .await
        .unwrap();
    (snapshot.state, page.items)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_ready_grant_attach_is_atomic_multiple_config_grants_share_one_binding() {
    let mut f = Fixture::new().await;
    f.record = f.ready().await;
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
    let original = f.record.clone();
    let agent = original.placement.target().agent_id.clone();
    let before = history(&f.resources, &agent).await;
    assert!(before.1.is_empty());
    let (store, service) = invokes(&f).await;
    let control_plane = Arc::new(f.lease);
    f.client
        .database(&f.database)
        .run_command(doc! {
            "collMod":"channel_usage_events", "validator":{"forced_failure":true},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    assert!(service
        .issue_channel(
            control_plane.clone(),
            &original.id,
            &original.pin,
            f.authority.clone()
        )
        .await
        .is_err());
    assert_eq!(history(&f.resources, &agent).await, before);
    for name in [
        "bridge_invoke_grants",
        "bridge_invoke_receipts",
        "bridge_invoke_audits",
    ] {
        assert_eq!(
            f.client
                .database(&f.database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0,
            "{name}"
        );
    }
    f.client
        .database(&f.database)
        .run_command(doc! {"collMod":"channel_usage_events","validator":{}})
        .await
        .unwrap();
    let first = service
        .issue_channel(
            control_plane.clone(),
            &original.id,
            &original.pin,
            f.authority.clone(),
        )
        .await
        .unwrap();
    let (attached, events) = history(&f.resources, &agent).await;
    assert_eq!(attached.active_bindings.len(), 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, control_plane.actor_context().actor);
    assert!(attached.first_used_at.is_none() && attached.last_used_at.is_none());
    let mut grants = vec![first];
    for number in 2..=3 {
        let bridge = f
            .bridges
            .bridge(&original.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        f.svc
            .mutate(
                &caller(),
                None,
                change(
                    &bridge.id,
                    bridge.revision,
                    &format!("usage-disable-{number}"),
                    BridgeChange::Disable {
                        accepted_work: AcceptedWorkPolicy::DrainOriginal,
                    },
                ),
            )
            .await
            .unwrap();
        let bridge = f
            .bridges
            .bridge(&original.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let mut updated = original.config.clone();
        updated.display_name = format!("New inbound configuration {number}");
        f.svc
            .mutate(
                &caller(),
                None,
                change(
                    &bridge.id,
                    bridge.revision,
                    &format!("usage-update-{number}"),
                    BridgeChange::Update {
                        config: Box::new(updated),
                        previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                    },
                ),
            )
            .await
            .unwrap();
        let bridge = f
            .bridges
            .bridge(&original.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
        f.svc
            .mutate(
                &caller(),
                None,
                change(
                    &bridge.id,
                    bridge.revision,
                    &format!("usage-enable-{number}"),
                    BridgeChange::Enable,
                ),
            )
            .await
            .unwrap();
        f.authority = renew_retained(&f.svc, &f.bridges, &original).await.unwrap();
        let inbound = f.authority.bridge().admission_pin().unwrap();
        let next = service
            .issue_channel(
                control_plane.clone(),
                &original.id,
                &inbound,
                f.authority.clone(),
            )
            .await
            .unwrap();
        assert_ne!(next.id, grants[0].id);
        if number == 2 {
            assert_eq!(
                history(&f.resources, &agent).await,
                (attached.clone(), events.clone())
            );
            grants.push(next);
            for (i, grant) in grants.iter().enumerate() {
                service
                    .revoke(
                        control_plane.actor_context(),
                        Some(control_plane.credential()),
                        RevokeBridgeInvokeGrant {
                            grant_id: grant.id.clone(),
                            expected_bridge_revision: f.authority.bridge().revision,
                            expected_revision: grant.revision,
                            idempotency_key: format!("usage-revoke-{i}"),
                        },
                    )
                    .await
                    .unwrap();
                let (current, current_events) = history(&f.resources, &agent).await;
                if i == 0 {
                    assert_eq!(current, attached);
                    assert_eq!(current_events, events);
                } else {
                    assert!(current.active_bindings.is_empty());
                    assert_eq!(current_events.len(), 2);
                    assert_eq!(current_events[1].actor, control_plane.actor_context().actor);
                }
            }
        } else {
            let (reattached, next_events) = history(&f.resources, &agent).await;
            assert_eq!(next_events.len(), 3);
            assert_eq!(reattached.active_bindings.len(), 1);
            let reference = &reattached.active_bindings[0].reference;
            let previous = &attached.active_bindings[0].reference;
            assert_ne!(reference.binding_id, previous.binding_id);
            assert_eq!(
                reference.previous_binding_id.as_ref(),
                Some(&previous.binding_id)
            );
            assert_eq!(reference.generation, previous.generation + 1);
            assert_eq!(reattached.creation, before.0.creation);
            assert!(
                store
                    .grant(&next.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .channel
                    .as_ref()
                    .unwrap()
                    .allocation
                    == original
            );
        }
    }
    assert!(
        f.allocations
            .generation(&original.identity)
            .await
            .unwrap()
            .unwrap()
            == original
    );
    f.client.database(&f.database).drop().await.unwrap();
}

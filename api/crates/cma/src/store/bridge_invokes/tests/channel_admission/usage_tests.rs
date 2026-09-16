//! Usage positives enter through genuine signed callbacks and real allocations.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::bridge::{allocation::*, ingress::CallbackInboxStore, partition::*};
use layer2_core::sandbox::{
    channel_usage::{
        AgentChannelUsageState, ChannelBindingChangeReason, ChannelUsageChange, ChannelUsageEvent,
    },
    store::{AgentChannelUsageStore, PageRequest},
};

async fn history(c: &ChannelFixture) -> (AgentChannelUsageState, Vec<ChannelUsageEvent>) {
    let (snapshot, page) = c
        .f
        .resources
        .agent_channel_usage_history(&c.f.target.agent_id, &PageRequest::new(100, None).unwrap())
        .await
        .unwrap();
    (snapshot.state, page.items)
}

async fn next_inbox(c: &mut ChannelFixture, number: i64) {
    let validation =
        c.f.authority
            .validate_connection(&c.f.bridge.owner, &c.f.bridge.current().unwrap().config)
            .await
            .unwrap();
    let write = fixtures::signed_inbox_named(
        &c.f.bridge,
        validation,
        &format!("external-{number}"),
        &format!("callback-{number}"),
        number,
    )
    .await;
    c.inbox = write.record().clone();
    let key = crate::store::AuthEncryptionKey::from_base64(&STANDARD.encode([7_u8; 32])).unwrap();
    crate::store::MongoCallbackInbox::open(&c.f.bridges, &c.f.database, "fixture-deployment", &key)
        .await
        .unwrap()
        .accept(write)
        .await
        .unwrap();
}

async fn collection_count(c: &ChannelFixture, name: &str) -> u64 {
    c.f.client
        .database(&c.f.database)
        .collection::<Document>(name)
        .count_documents(doc! {})
        .await
        .unwrap()
}

async fn force_event_failure(c: &ChannelFixture, enabled: bool) {
    c.f.client
        .database(&c.f.database)
        .run_command(doc! {
            "collMod":"channel_usage_events",
            "validator": if enabled { doc! {"forced_failure":true} } else { doc! {} },
            "validationLevel":"strict","validationAction":"error"
        })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_signed_admission_later_use_and_old_replay_keep_original_events_and_creation()
{
    let mut c = ChannelFixture::new().await;
    let (before, events) = history(&c).await;
    assert!(events.is_empty() && before.active_bindings.is_empty());
    let logs =
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 50)
            .await
            .unwrap();
    let original = c.inbox.clone();
    let one = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (after_one, one_events) = history(&c).await;
    assert_eq!(one_events.len(), 2);
    assert!(matches!(
        one_events[0].change,
        ChannelUsageChange::Attach { .. }
    ));
    assert_eq!(one_events[0].actor, one.receipt.envelope.actor);
    assert_eq!(one_events[1].actor, one.receipt.envelope.actor);
    assert_eq!(
        one_events[1].reason,
        ChannelBindingChangeReason::AcceptedCommand
    );
    assert_eq!(after_one.first_used_at.as_ref(), Some(&one_events[1].at));
    next_inbox(&mut c, 43).await;
    let two = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    assert_eq!(two.receipt.agent_order, one.receipt.agent_order + 1);
    let (after_two, two_events) = history(&c).await;
    assert_eq!(two_events.len(), 3);
    assert_eq!(&two_events[..2], &one_events);
    assert_eq!(after_two.last_used_at.as_ref(), Some(&two_events[2].at));
    assert_eq!(after_two.first_used_at, after_one.first_used_at);
    assert_eq!(after_two.active_bindings.len(), 1);
    c.inbox = original;
    let reopened = MongoChannelCommandStore::new(Arc::new(
        MongoBridgeInvokeStore::open(c.f.bridges.clone(), c.f.resources.clone(), &c.f.database)
            .await
            .unwrap(),
    ));
    assert_eq!(
        reopened
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        one
    );
    assert_eq!(history(&c).await, (after_two.clone(), two_events));
    assert_eq!(after_two.creation, before.creation);
    assert_eq!(
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 50)
            .await
            .unwrap(),
        logs
    );
    assert_eq!(collection_count(&c, "agent_commands").await, 2);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_event_failure_rolls_back_command_order_inbox_and_both_binding_links() {
    let c = ChannelFixture::new().await;
    let before = history(&c).await;
    let ui = ui_command(&c.f, "before-usage-failure").await;
    force_event_failure(&c, true).await;
    assert!(c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .is_err());
    assert_eq!(history(&c).await, before);
    assert_eq!(collection_count(&c, "agent_commands").await, 1);
    for (name, id, field) in [
        (
            "bridge_callback_inbox",
            c.inbox.id.as_str(),
            "command_correlation",
        ),
        (
            "bridge_allocations",
            c.allocation.id.as_str(),
            "channel_usage_binding",
        ),
        (
            "bridge_invoke_grants",
            c.grant.id.as_str(),
            "channel_usage_binding",
        ),
    ] {
        let document =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(name)
                .find_one(doc! {"_id":id})
                .await
                .unwrap()
                .unwrap();
        assert!(!document.contains_key(field), "{name}.{field}");
    }
    force_event_failure(&c, false).await;
    let accepted = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    assert_eq!(accepted.receipt.agent_order, ui.receipt.agent_order + 1);
    assert_eq!(history(&c).await.1.len(), 2);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_revoke_failure_rolls_back_grant_receipt_audit_and_detach() {
    let c = ChannelFixture::new().await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    let request = c.f.revoke(&c.grant, "usage-revoke");
    force_event_failure(&c, true).await;
    assert!(c
        .f
        .service
        .revoke(&caller(), None, request.clone())
        .await
        .is_err());
    assert_eq!(history(&c).await, before);
    assert_eq!(
        c.f.store.grant(&c.grant.id).await.unwrap().unwrap().view(),
        c.grant
    );
    for name in ["bridge_invoke_receipts", "bridge_invoke_audits"] {
        assert_eq!(collection_count(&c, name).await, 1, "{name}");
    }
    force_event_failure(&c, false).await;
    let revoked =
        c.f.service
            .revoke(&caller(), None, request.clone())
            .await
            .unwrap();
    assert_eq!(
        c.f.service.revoke(&caller(), None, request).await.unwrap(),
        revoked
    );
    let (after, events) = history(&c).await;
    assert!(after.active_bindings.is_empty());
    assert_eq!(after.creation, before.0.creation);
    assert_eq!(after.first_used_at, before.0.first_used_at);
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].actor, caller().actor);
    assert_eq!(events[2].reason, ChannelBindingChangeReason::Detachment);
    c.close().await;
}

async fn reset(c: &ChannelFixture) -> AllocationReservation {
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
            .management_lease(&caller(), None, &c.f.bridge.id, c.f.bridge.revision)
            .await
            .unwrap();
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
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_reset_is_atomic_and_next_signed_input_rebinds_same_agent_with_predecessor() {
    let mut c = ChannelFixture::new().await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    let old = c.allocation.clone();
    let store = crate::store::MongoBridgeAllocationStore::open(
        c.f.bridges.clone(),
        Some(c.f.store.clone()),
        &c.f.database,
    )
    .await
    .unwrap();
    force_event_failure(&c, true).await;
    assert!(store.reserve(reset(&c).await).await.is_err());
    assert!(store.generation(&old.identity).await.unwrap().unwrap() == old);
    assert_eq!(history(&c).await, before);
    force_event_failure(&c, false).await;
    c.allocation = store.reserve(reset(&c).await).await.unwrap();
    let (detached, reset_events) = history(&c).await;
    assert!(detached.active_bindings.is_empty());
    assert_eq!(reset_events.len(), 3);
    assert_eq!(reset_events[2].reason, ChannelBindingChangeReason::Reset);
    assert_eq!(reset_events[2].actor, caller().actor);
    assert_eq!(
        store
            .generation(&old.identity)
            .await
            .unwrap()
            .unwrap()
            .state,
        AllocationState::Retired
    );
    assert_eq!(c.allocation.predecessor.as_ref().unwrap().id, old.id);
    next_inbox(&mut c, 44).await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let (after, events) = history(&c).await;
    assert_eq!(events.len(), 5);
    assert_eq!(after.creation, before.0.creation);
    assert_eq!(after.active_bindings.len(), 1);
    let binding = &after.active_bindings[0].reference;
    let original = &before.0.active_bindings[0].reference;
    assert_ne!(binding.binding_id, original.binding_id);
    assert_eq!(binding.generation, original.generation + 1);
    assert_eq!(
        binding.previous_binding_id.as_ref(),
        Some(&original.binding_id)
    );
    c.f.service
        .revoke(&caller(), None, c.f.revoke(&c.grant, "new-binding-revoke"))
        .await
        .unwrap();
    assert!(history(&c).await.0.active_bindings.is_empty());
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_substituted_binding_sidecar_refuses_without_partial_new_command() {
    let mut c = ChannelFixture::new().await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    c.f.store
        .grants
        .update_one(
            doc! {"_id":c.grant.id.as_str()},
            doc! {"$set":{"channel_usage_binding.agent_created_at":"2000-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    next_inbox(&mut c, 45).await;
    assert!(c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .is_err());
    assert_eq!(history(&c).await, before);
    assert_eq!(collection_count(&c, "agent_commands").await, 1);
    let inbox =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id":&c.inbox.id})
            .await
            .unwrap()
            .unwrap();
    assert!(!inbox.contains_key("command_correlation"));
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_manager_revoke_preserves_historical_identity_after_target_removal_or_recreation(
) {
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
        let request = c.f.revoke(&c.grant, "removed-target-revoke");
        let revoked =
            c.f.service
                .revoke(&caller(), None, request.clone())
                .await
                .unwrap();
        assert_eq!(
            c.f.service.revoke(&caller(), None, request).await.unwrap(),
            revoked
        );
        assert_eq!(
            c.f.resources
                .agents
                .find_one(doc! {"_id":id})
                .await
                .unwrap(),
            target_before
        );
        let latest =
            c.f.resources
                .channel_usage_events
                .find_one(doc! {"agent_id":id})
                .sort(doc! {"revision":-1})
                .await
                .unwrap()
                .unwrap();
        let event: ChannelUsageEvent =
            mongodb::bson::deserialize_from_bson(latest.get("record").unwrap().clone()).unwrap();
        event.validate().unwrap();
        assert_eq!(event.before, before.0);
        assert_eq!(event.after.creation, before.0.creation);
        assert!(event.after.active_bindings.is_empty());
        assert_eq!(event.actor, caller().actor);
        assert_eq!(collection_count(&c, "channel_usage_events").await, 3);
        c.close().await;
    }
}

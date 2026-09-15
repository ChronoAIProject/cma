use super::*;

#[tokio::test]
async fn channel_usage_historical_use_requires_exact_evidence_and_keeps_siblings_active() {
    let store = ready_store(3).await;
    let creation = created(
        store
            .admit_agent(admission("historical-use"))
            .await
            .unwrap(),
    );
    let id = &creation.agent.id;
    let mut sibling = reference("sibling-binding", 1, None);
    sibling.bridge_id = "sibling-bridge".into();
    append(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: sibling.clone(),
        },
        ChannelBindingChangeReason::Attachment,
        "sibling",
    )
    .await;
    let original = store.agent_channel_usage_snapshot(id).await.unwrap();
    let binding = reference("historical-binding", 1, None);
    let mut request = mutation(
        &store,
        id,
        ChannelUsageChange::Use {
            binding_id: binding.binding_id.clone(),
        },
        ChannelBindingChangeReason::AcceptedCommand,
        "late-use",
    )
    .await;
    request.actor = ResourceActor::Channel {
        bridge_id: binding.bridge_id.clone(),
        grant_id: "actual-grant".into(),
        platform: "telegram".into(),
        chat_id: "chat".into(),
        sender_id: "sender".into(),
    };
    assert!(plan_channel_usage(&request, &original.provenance, &original.state, None).is_err());
    assert!(plan_channel_usage(
        &request,
        &original.provenance,
        &original.state,
        Some(sibling.clone())
    )
    .is_err());
    let event = plan_channel_usage(
        &request,
        &original.provenance,
        &original.state,
        Some(binding.clone()),
    )
    .unwrap();
    event.validate().unwrap();
    assert_eq!(event.after.active_bindings, original.state.active_bindings);
    assert_eq!(event.after.creation, original.state.creation);
    assert_eq!(event.after.first_used_at.as_ref(), Some(&request.at));
    assert!(event.detached_at.is_none());
    assert_eq!(request.replay(event.clone()).unwrap(), event);
    let mut invalid = event.clone();
    invalid.predecessor = None;
    assert!(invalid.validate().is_err());
    let mut wrong_actor = request.clone();
    wrong_actor.actor = ResourceActor::Channel {
        bridge_id: "another-bridge".into(),
        grant_id: "grant".into(),
        platform: "telegram".into(),
        chat_id: "chat".into(),
        sender_id: "sender".into(),
    };
    assert!(plan_channel_usage(
        &wrong_actor,
        &original.provenance,
        &original.state,
        Some(binding.clone())
    )
    .is_err());
    let mut detached = request;
    detached.change = ChannelUsageChange::Detach {
        binding_id: binding.binding_id.clone(),
    };
    detached.reason = ChannelBindingChangeReason::Detachment;
    assert!(plan_channel_usage(
        &detached,
        &original.provenance,
        &original.state,
        Some(binding)
    )
    .is_err());
}

#[tokio::test]
async fn channel_usage_rebind_detach_preserves_original_actor_and_before_after() {
    let store = ready_store(3).await;
    let creation = created(store.admit_agent(admission("rebind-detach")).await.unwrap());
    let id = &creation.agent.id;
    let attached = attach(&store, id).await;
    let actor = ResourceActor::Api {
        subject: SubjectId::new("manager"),
        delegation_id: None,
    };
    let detached = plan_retained_channel_detachment(
        &attached.after,
        &reference("binding-one", 1, None),
        &actor,
        &"b".repeat(64),
        ChannelBindingChangeReason::Rebind,
        "2026-09-05T00:02:00Z",
    )
    .unwrap();
    assert_eq!(detached.before, attached.after);
    assert_eq!(detached.after.creation, attached.after.creation);
    assert!(detached.after.active_bindings.is_empty());
    assert_eq!(detached.actor, actor);
    assert_eq!(detached.reason, ChannelBindingChangeReason::Rebind);
    detached.validate().unwrap();
}

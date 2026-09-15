use super::*;
use crate::resource_access::{ResourceActor, ResourceOwner, ResourcePolicyRecord};
use crate::sandbox::{
    channel_usage::*,
    store::{AgentChannelUsageStore, PageRequest},
};

mod inventory_tests;
mod service_tests;

fn reference(id: &str, generation: u64, previous: Option<&str>) -> ChannelBindingReference {
    ChannelBindingReference {
        binding_id: id.into(),
        bridge_id: "bridge-one".into(),
        partition_id: "partition-one".into(),
        generation,
        previous_binding_id: previous.map(str::to_owned),
    }
}
async fn mutation(
    store: &InMemorySandboxStore,
    id: &AgentId,
    change: ChannelUsageChange,
    reason: ChannelBindingChangeReason,
    key: &str,
) -> ChannelUsageMutation {
    let snapshot = store.agent_channel_usage_snapshot(id).await.unwrap();
    ChannelUsageMutation {
        creation: snapshot.state.creation,
        policy_fence: snapshot.provenance.resource.fence(),
        expected_revision: snapshot.state.revision,
        key_sha256: crate::profiles::content_digest(&key).unwrap(),
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        change,
        reason,
        at: "2026-09-05T00:01:00Z".into(),
    }
}
async fn append(
    store: &InMemorySandboxStore,
    id: &AgentId,
    change: ChannelUsageChange,
    reason: ChannelBindingChangeReason,
    key: &str,
) -> ChannelUsageEvent {
    store
        .record_agent_channel_usage(mutation(store, id, change, reason, key).await)
        .await
        .unwrap()
}
async fn attach(store: &InMemorySandboxStore, id: &AgentId) -> ChannelUsageEvent {
    append(
        store,
        id,
        ChannelUsageChange::Attach {
            binding: reference("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach",
    )
    .await
}

#[tokio::test]
async fn channel_usage_manual_and_trigger_origins_survive_attach_use_reset_detach_and_retirement() {
    for origin in [
        AgentOrigin::Manual,
        AgentOrigin::Trigger {
            trigger_id: "trigger-one".into(),
            launch_id: "launch-one".into(),
        },
    ] {
        let store = ready_store(3).await;
        let mut request = admission("channel-usage-origin");
        request.origin = origin.clone();
        request.created_by = Some(ResourceActor::Human {
            subject: SubjectId::new("creator"),
        });
        let creation = created(store.admit_agent(request).await.unwrap());
        let id = &creation.agent.id;
        let original = store.agent_channel_usage_snapshot(id).await.unwrap();
        assert_eq!(original.state.summary(), ChannelUsageSummary::default());
        let attached = attach(&store, id).await;
        assert!(attached.after.summary().active);
        assert!(attached.after.summary().has_history);
        assert!(attached.after.summary().first_used_at.is_none());
        append(
            &store,
            id,
            ChannelUsageChange::Use {
                binding_id: "binding-one".into(),
            },
            ChannelBindingChangeReason::AcceptedCommand,
            "first-use",
        )
        .await;
        let reset = append(
            &store,
            id,
            ChannelUsageChange::Replace {
                before_binding_id: "binding-one".into(),
                binding: reference("binding-two", 2, Some("binding-one")),
            },
            ChannelBindingChangeReason::Reset,
            "reset",
        )
        .await;
        assert_eq!(reset.predecessor, Some(reference("binding-one", 1, None)));
        assert_eq!(
            reset.before.active_bindings[0].reference.binding_id,
            "binding-one"
        );
        assert_eq!(
            reset.after.active_bindings[0].reference.binding_id,
            "binding-two"
        );
        assert_eq!(
            reset.after.first_used_at.as_deref(),
            Some("2026-09-05T00:01:00Z")
        );
        assert!(reset.after.active_bindings[0].first_used_at.is_none());
        // Runtime/lifecycle and profile changes retain the independent facts.
        {
            let mut state = store.lock();
            state.agents.get_mut(id).unwrap().title = Some("later-title".into());
            state.agents.get_mut(id).unwrap().profile.revision += 1;
            state.agents.get_mut(id).unwrap().status = AgentStatus::Deleted;
            state
                .sandboxes
                .get_mut(&creation.agent.sandbox_id)
                .unwrap()
                .retired_at = Some("2026-09-05T00:01:00Z".into());
        }
        append(
            &store,
            id,
            ChannelUsageChange::Detach {
                binding_id: "binding-two".into(),
            },
            ChannelBindingChangeReason::AdministrativeIntervention,
            "detach",
        )
        .await;
        let (retained, history) = store
            .agent_channel_usage_history(id, &PageRequest::new(10, None).unwrap())
            .await
            .unwrap();
        assert!(!retained.state.summary().active);
        assert!(retained.state.summary().has_history);
        assert_eq!(retained.state.creation, original.state.creation);
        assert_eq!(retained.state.creation.provisioning_origin, origin);
        assert_eq!(history.items.len(), 4);
        assert_eq!(
            history.items[3].reason,
            ChannelBindingChangeReason::AdministrativeIntervention
        );
        assert_eq!(store.lock().agent_logs[id], vec![creation.initial_log]);
        assert_eq!(
            store.lock().agents[id].conversation,
            creation.agent.conversation
        );
        assert!(store.lock().resource_shares.is_empty());
    }
}

#[tokio::test]
async fn channel_usage_concurrent_retry_and_later_replay_keep_exact_original_event() {
    let store = ready_store(3).await;
    let creation = created(store.admit_agent(admission("usage-retry")).await.unwrap());
    let id = &creation.agent.id;
    let request = mutation(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: reference("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "stable-key",
    )
    .await;
    let (left, right) = tokio::join!(
        store.record_agent_channel_usage(request.clone()),
        store.record_agent_channel_usage(request.clone())
    );
    let event = left.unwrap();
    assert_eq!(right.unwrap(), event);
    append(
        &store,
        id,
        ChannelUsageChange::Detach {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::Detachment,
        "detach",
    )
    .await;
    let mut retry = request.clone();
    retry.at = "2026-09-05T00:09:00Z".into();
    assert_eq!(
        store.record_agent_channel_usage(retry).await.unwrap(),
        event
    );
    let mut changed = request;
    changed.actor = ResourceActor::Human {
        subject: SubjectId::new("different-actor"),
    };
    assert_eq!(
        store.record_agent_channel_usage(changed).await.unwrap_err(),
        SandboxStoreError::Conflict
    );
    let state = store.lock();
    assert_eq!(state.channel_usage_events.len(), 2);
    assert_eq!(state.agent_channel_usage[id].state.revision, 2);
    assert!(state.agent_channel_usage[id]
        .state
        .active_bindings
        .is_empty());
}

#[tokio::test]
async fn channel_usage_policy_revocation_blocks_stale_append_and_stale_replay() {
    let store = ready_store(3).await;
    let creation = created(store.admit_agent(admission("usage-revoke")).await.unwrap());
    let id = &creation.agent.id;
    let request = mutation(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: reference("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach",
    )
    .await;
    let event = store
        .record_agent_channel_usage(request.clone())
        .await
        .unwrap();
    let use_request = mutation(
        &store,
        id,
        ChannelUsageChange::Use {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::AcceptedCommand,
        "use",
    )
    .await;
    store.lock().resource_policies.insert(
        creation.agent.sandbox_id.clone(),
        ResourcePolicyRecord {
            sandbox_id: creation.agent.sandbox_id.clone(),
            sandbox_created_at: NOW.into(),
            owner: ResourceOwner::Person {
                subject: SubjectId::new("owner"),
            },
            organization_evidence: None,
            revision: 1,
        },
    );
    assert_eq!(
        store
            .record_agent_channel_usage(use_request)
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    assert_eq!(
        store
            .record_agent_channel_usage(request.clone())
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    let mut fresh = request;
    fresh.policy_fence = store
        .agent_channel_usage_snapshot(id)
        .await
        .unwrap()
        .provenance
        .resource
        .fence();
    assert_eq!(
        store.record_agent_channel_usage(fresh).await.unwrap(),
        event
    );
    assert_eq!(store.lock().channel_usage_events.len(), 1);
}

#[tokio::test]
async fn channel_usage_invalid_lineage_duplicate_binding_and_capacity_leave_no_partial_facts() {
    let store = ready_store(3).await;
    let creation = created(store.admit_agent(admission("usage-invalid")).await.unwrap());
    let id = &creation.agent.id;
    attach(&store, id).await;
    for index in 1..MAX_ACTIVE_CHANNEL_BINDINGS {
        let mut binding = reference(&format!("binding-{index}"), 1, None);
        binding.partition_id = format!("partition-{index}");
        append(
            &store,
            id,
            ChannelUsageChange::Attach { binding },
            ChannelBindingChangeReason::Attachment,
            &format!("attach-{index}"),
        )
        .await;
    }
    let before = store.lock().clone();
    for binding in [
        reference("overflow", 1, None),
        reference("skipped-generation", 3, Some("binding-one")),
    ] {
        let request = mutation(
            &store,
            id,
            ChannelUsageChange::Attach { binding },
            ChannelBindingChangeReason::Attachment,
            "invalid",
        )
        .await;
        assert!(store.record_agent_channel_usage(request).await.is_err());
    }
    let mut forged = mutation(
        &store,
        id,
        ChannelUsageChange::Use {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::AcceptedCommand,
        "forged",
    )
    .await;
    forged.creation.creation_origin = crate::sandbox::AgentCreationOrigin::LegacyUnknown;
    assert!(store.record_agent_channel_usage(forged).await.is_err());
    let state = store.lock();
    assert_eq!(state.agent_channel_usage, before.agent_channel_usage);
    assert_eq!(state.channel_usage_events, before.channel_usage_events);
    assert_eq!(state.agents, before.agents);
    assert_eq!(state.resource_shares, before.resource_shares);
}

#[tokio::test]
async fn channel_usage_history_checks_hidden_lookahead_gaps_and_latest_summary() {
    let store = ready_store(3).await;
    let creation = created(
        store
            .admit_agent(admission("usage-corruption"))
            .await
            .unwrap(),
    );
    let id = &creation.agent.id;
    attach(&store, id).await;
    append(
        &store,
        id,
        ChannelUsageChange::Use {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::AcceptedCommand,
        "use",
    )
    .await;
    append(
        &store,
        id,
        ChannelUsageChange::Detach {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::Detachment,
        "detach",
    )
    .await;
    let page = PageRequest::new(1, None).unwrap();
    let first = store
        .agent_channel_usage_history(id, &page)
        .await
        .unwrap()
        .1;
    assert_eq!(first.next.as_deref(), Some("1"));
    let final_page = store
        .agent_channel_usage_history(id, &PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap()
        .1;
    assert_eq!(final_page.items.len(), 2);
    assert!(final_page.next.is_none());
    let key = (id.clone(), 2);
    let original = store.lock().channel_usage_events[&key].clone();
    store
        .lock()
        .channel_usage_events
        .get_mut(&key)
        .unwrap()
        .actor = ResourceActor::Human {
        subject: SubjectId::new("forged"),
    };
    assert!(matches!(
        store.agent_channel_usage_history(id, &page).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store.lock().channel_usage_events.remove(&key);
    assert!(matches!(
        store.agent_channel_usage_history(id, &page).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store.lock().channel_usage_events.insert(key, original);
    store
        .lock()
        .agent_channel_usage
        .get_mut(id)
        .unwrap()
        .state
        .last_used_at = None;
    assert!(matches!(
        store.agent_channel_usage_snapshot(id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
}

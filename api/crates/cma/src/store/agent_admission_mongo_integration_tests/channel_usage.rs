use super::*;
use layer2_core::resource_access::ResourceActor;
use layer2_core::sandbox::{
    channel_usage::*,
    store::{AgentChannelUsageStore, PageRequest},
};

fn binding(id: &str, generation: u64, previous: Option<&str>) -> ChannelBindingReference {
    ChannelBindingReference {
        binding_id: id.into(),
        bridge_id: "bridge-one".into(),
        partition_id: "partition-one".into(),
        generation,
        previous_binding_id: previous.map(str::to_owned),
    }
}
async fn mutation(
    store: &MongoSandboxStore,
    id: &AgentId,
    change: ChannelUsageChange,
    reason: ChannelBindingChangeReason,
    key: &str,
) -> ChannelUsageMutation {
    let snapshot = store.agent_channel_usage_snapshot(id).await.unwrap();
    ChannelUsageMutation {
        policy_fence: snapshot.provenance.resource.fence(),
        creation: snapshot.state.creation,
        expected_revision: snapshot.state.revision,
        key_sha256: layer2_core::profiles::content_digest(&key).unwrap(),
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        change,
        reason,
        at: "2026-09-05T00:01:00Z".into(),
    }
}
async fn append(
    store: &MongoSandboxStore,
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_usage_concurrent_retries_reopen_and_detach_retain_origin_and_complete_history() {
    let (client, database, store, _, _, mut request) = fixture("channel_usage_retry").await;
    request.origin = AgentOrigin::Trigger {
        trigger_id: "origin-trigger".into(),
        launch_id: "origin-launch".into(),
    };
    let creation = store.admit_agent(request).await.unwrap();
    let id = &creation.creation().agent.id;
    let before = store.agent_channel_usage_snapshot(id).await.unwrap();
    let input = mutation(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: binding("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach",
    )
    .await;
    let (left, right) = tokio::join!(
        store.record_agent_channel_usage(input.clone()),
        store.record_agent_channel_usage(input.clone())
    );
    let original = left.unwrap();
    assert_eq!(right.unwrap(), original);
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
        ChannelUsageChange::Replace {
            before_binding_id: "binding-one".into(),
            binding: binding("binding-two", 2, Some("binding-one")),
        },
        ChannelBindingChangeReason::Rebind,
        "rebind",
    )
    .await;
    append(
        &store,
        id,
        ChannelUsageChange::Detach {
            binding_id: "binding-two".into(),
        },
        ChannelBindingChangeReason::Detachment,
        "detach",
    )
    .await;
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut retry = input;
    retry.at = "2026-09-05T00:05:00Z".into();
    assert_eq!(
        reopened.record_agent_channel_usage(retry).await.unwrap(),
        original
    );
    let (after, page) = reopened
        .agent_channel_usage_history(id, &PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(after.state.creation, before.state.creation);
    assert!(!after.state.summary().active);
    assert!(after.state.summary().has_history);
    assert_eq!(
        after.state.first_used_at.as_deref(),
        Some("2026-09-05T00:01:00Z")
    );
    assert_eq!(page.next.as_deref(), Some("2"));
    let last = reopened
        .agent_channel_usage_history(id, &PageRequest::new(2, page.next).unwrap())
        .await
        .unwrap()
        .1;
    assert_eq!(last.items.len(), 2);
    assert!(last.next.is_none());
    assert_eq!(
        last.items[1].before.active_bindings[0].reference.generation,
        2
    );
    assert_eq!(
        reopened
            .channel_usage_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        4
    );
    assert_eq!(
        reopened.get_agent(id).await.unwrap(),
        Some(creation.creation().agent.clone())
    );
    assert_eq!(
        reopened
            .agent_log
            .count_documents(doc! {"agent_id":id.as_str()})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .resource_shares
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_usage_transaction_abort_late_conflict_and_policy_change_leave_no_partial_facts() {
    let (client, database, store, parent, _, request) = fixture("channel_usage_abort").await;
    let creation = store.admit_agent(request).await.unwrap();
    let id = &creation.creation().agent.id;
    let input = mutation(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: binding("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach",
    )
    .await;
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    crate::store::agent_channel_usage::apply(&store, &input, &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .channel_usage_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .agent_channel_usage
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let original = store
        .record_agent_channel_usage(input.clone())
        .await
        .unwrap();
    let mut duplicate = input.clone();
    duplicate.key_sha256 = "b".repeat(64);
    assert_eq!(
        store
            .record_agent_channel_usage(duplicate)
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    // A share mutation increments this exact durable policy fence; a stale
    // accepted-command request or replay must stop before an append.
    store.resource_policies.insert_one(doc! {
        "_id":parent.id.as_str(),"document_schema_version":1_i64,"revision":1_i64,
        "owner_kind":"person","owner_subject":parent.owner.as_str(),
        "record":{"sandbox_id":parent.id.as_str(),"sandbox_created_at":&parent.created_at,
        "owner":{"kind":"person","subject":parent.owner.as_str()},"organization_evidence":mongodb::bson::Bson::Null,"revision":1_i64}
    }).await.unwrap();
    assert_eq!(
        store.record_agent_channel_usage(input).await.unwrap_err(),
        SandboxStoreError::Conflict
    );
    assert_eq!(
        store
            .channel_usage_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.agent_channel_usage_snapshot(id).await.unwrap().state,
        original.after
    );
    assert_eq!(
        store.get_agent(id).await.unwrap(),
        Some(creation.creation().agent.clone())
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_usage_corrupt_lookahead_missing_event_and_summary_refuse_reads_after_reopen() {
    let (client, database, store, _, _, request) = fixture("channel_usage_corrupt").await;
    let creation = store.admit_agent(request).await.unwrap();
    let id = &creation.creation().agent.id;
    append(
        &store,
        id,
        ChannelUsageChange::Attach {
            binding: binding("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach",
    )
    .await;
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
    let filter = doc! {"agent_id":id.as_str(),"revision":2_i64};
    let original = store
        .channel_usage_events
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    store
        .channel_usage_events
        .update_one(
            filter.clone(),
            doc! {"$set":{"record.actor.subject":"forged-actor"}},
        )
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let page = PageRequest::new(1, None).unwrap();
    assert!(matches!(
        reopened.agent_channel_usage_history(id, &page).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store.channel_usage_events.delete_one(filter).await.unwrap();
    assert!(matches!(
        reopened.agent_channel_usage_history(id, &page).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .channel_usage_events
        .insert_one(original)
        .await
        .unwrap();
    store
        .agent_channel_usage
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"record.state.last_used_at":mongodb::bson::Bson::Null}},
        )
        .await
        .unwrap();
    assert!(matches!(
        reopened.agent_channel_usage_snapshot(id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        store.get_agent(id).await.unwrap(),
        Some(creation.creation().agent.clone())
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_usage_inventory_reopens_filters_without_relabeling_and_refuses_hidden_corruption()
{
    use layer2_core::sandbox::{AgentCreationCategory, AgentProvenanceInventoryQuery};
    let (client, database, store, _, _, mut request) = fixture("channel_usage_inventory").await;
    request.agent_id = AgentId("agt_channel_manual".into());
    let first = store
        .admit_agent(request.clone())
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    append(
        &store,
        &first.id,
        ChannelUsageChange::Attach {
            binding: binding("binding-one", 1, None),
        },
        ChannelBindingChangeReason::Attachment,
        "attach-one",
    )
    .await;
    append(
        &store,
        &first.id,
        ChannelUsageChange::Detach {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::Detachment,
        "detach-one",
    )
    .await;
    request.agent_id = AgentId("agt_channel_trigger".into());
    request.worktree_id = WorktreeId::generate();
    request.creation_key = "trigger-inventory".into();
    request.origin = AgentOrigin::Trigger {
        trigger_id: "original-trigger".into(),
        launch_id: "original-launch".into(),
    };
    let second = store
        .admit_agent(request)
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    let mut reference = binding("binding-two", 1, None);
    reference.partition_id = "partition-two".into();
    append(
        &store,
        &second.id,
        ChannelUsageChange::Attach { binding: reference },
        ChannelBindingChangeReason::Attachment,
        "attach-two",
    )
    .await;
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let service = super::provenance_inventory::service(reopened.clone());
    let owner = super::provenance_inventory::owner();
    let active = service
        .list_personal(
            &owner,
            &AgentProvenanceInventoryQuery {
                channel_active: Some(true),
                creation_origin: Some(AgentCreationCategory::Trigger),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(active.items.len(), 1);
    assert_eq!(active.items[0].agent_id, second.id);
    assert!(active.items[0].channel_usage.active);
    let detached = service
        .list_personal(
            &owner,
            &AgentProvenanceInventoryQuery {
                channel_active: Some(false),
                channel_history: Some(true),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detached.items[0].agent_id, first.id);
    assert_eq!(
        detached.items[0].creation_origin,
        AgentCreationCategory::Manual
    );
    assert!(detached.items[0].channel_usage.has_history);
    assert!(!detached.items[0].channel_usage.active);
    reopened
        .agent_channel_usage
        .update_one(
            doc! {"_id":second.id.as_str()},
            doc! {"$set":{"record.latest_event_digest":"corrupt"}},
        )
        .await
        .unwrap();
    assert!(service
        .list_personal(
            &owner,
            &AgentProvenanceInventoryQuery {
                channel_active: Some(false),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap()
        )
        .await
        .is_err());
    assert_eq!(reopened.get_agent(&first.id).await.unwrap(), Some(first));
    assert_eq!(reopened.get_agent(&second.id).await.unwrap(), Some(second));
    assert_eq!(
        reopened
            .channel_usage_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    client.database(&database).drop().await.unwrap();
}

#[path = "channel_usage/uncertain_commit.rs"]
mod uncertain_commit;

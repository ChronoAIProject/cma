use super::*;
use crate::resource_access::*;
use crate::sandbox::{
    AgentCreationCategory, AgentProvenanceInventoryQuery, AgentProvenanceInventoryService,
};
use crate::{Authorization, Caller, Capability};

struct NoOrganization;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganization {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}
fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn service(store: Arc<InMemorySandboxStore>) -> Arc<AgentProvenanceInventoryService> {
    AgentProvenanceInventoryService::new(
        store.clone(),
        Arc::new(ResourceAccessService::new(store, Arc::new(NoOrganization))),
    )
}

#[tokio::test]
async fn channel_inventory_filters_advance_bounded_empty_pages_and_keep_creation_categories() {
    let store = Arc::new(ready_store(3).await);
    for index in 0..202 {
        let mut request = admission(&format!("inventory-{index}"));
        request.agent_id = AgentId(format!("agt_usage_{index:03}"));
        if index == 200 {
            request.origin = AgentOrigin::Trigger {
                trigger_id: "original-trigger".into(),
                launch_id: "original-launch".into(),
            };
        }
        let id = created(store.admit_agent(request).await.unwrap()).agent.id;
        if index >= 200 {
            let mut binding = reference(&format!("binding-{index}"), 1, None);
            binding.partition_id = format!("partition-{index}");
            append(
                &store,
                &id,
                ChannelUsageChange::Attach { binding },
                ChannelBindingChangeReason::Attachment,
                &format!("attach-{index}"),
            )
            .await;
            if index == 201 {
                append(
                    &store,
                    &id,
                    ChannelUsageChange::Detach {
                        binding_id: format!("binding-{index}"),
                    },
                    ChannelBindingChangeReason::Detachment,
                    "detach",
                )
                .await;
            }
        }
        store.lock().agents.get_mut(&id).unwrap().status = AgentStatus::Deleted;
    }
    let before = store.lock().clone();
    let service = service(store.clone());
    let query = AgentProvenanceInventoryQuery {
        channel_active: Some(true),
        creation_origin: Some(AgentCreationCategory::Trigger),
        ..Default::default()
    };
    let first = service
        .list_personal(&owner(), &query, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next.as_deref(), Some("agt_usage_199"));
    let last = service
        .list_personal(&owner(), &query, &PageRequest::new(1, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(last.items[0].agent_id.as_str(), "agt_usage_200");
    assert!(last.items[0].channel_usage.active);
    assert_eq!(
        last.items[0].creation_origin,
        AgentCreationCategory::Trigger
    );
    assert!(last.next.is_none());
    let detached = service
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery {
                channel_active: Some(false),
                channel_history: Some(true),
                ..Default::default()
            },
            &PageRequest::new(2, Some("agt_usage_199".into())).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detached.items.len(), 1);
    assert_eq!(detached.items[0].agent_id.as_str(), "agt_usage_201");
    assert_eq!(
        detached.items[0].creation_origin,
        AgentCreationCategory::Manual
    );
    assert!(detached.items[0].channel_usage.has_history);
    assert!(!detached.items[0].channel_usage.active);
    let unused = service
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery {
                channel_history: Some(false),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unused.items[0].agent_id.as_str(), "agt_usage_000");
    assert_eq!(unused.next.as_deref(), Some("agt_usage_000"));
    let state = store.lock();
    assert_eq!(state.agents, before.agents);
    assert_eq!(state.agent_logs, before.agent_logs);
    assert_eq!(state.agent_channel_usage, before.agent_channel_usage);
    assert_eq!(state.channel_usage_events, before.channel_usage_events);
}

#[tokio::test]
async fn channel_inventory_refuses_corrupt_usage_on_an_excluded_lookahead() {
    let store = Arc::new(ready_store(3).await);
    let mut first = admission("first");
    first.agent_id = AgentId("agt_first".into());
    let mut second = admission("second");
    second.agent_id = AgentId("agt_second".into());
    store.admit_agent(first).await.unwrap();
    let second = created(store.admit_agent(second).await.unwrap()).agent.id;
    attach(&store, &second).await;
    store
        .lock()
        .agent_channel_usage
        .get_mut(&second)
        .unwrap()
        .latest_event_digest = "bad".into();
    let result = service(store)
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery {
                channel_history: Some(false),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap(),
        )
        .await;
    assert!(matches!(
        result,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        ))
    ));
}

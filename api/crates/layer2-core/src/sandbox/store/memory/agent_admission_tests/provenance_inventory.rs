use super::*;
use crate::resource_access::*;
use crate::sandbox::store::{AgentProvenanceInventoryStore, PageRequest};
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
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoOrganization),
    ));
    AgentProvenanceInventoryService::new(store, access)
}

async fn create(store: &InMemorySandboxStore, index: usize, origin: AgentOrigin) -> AgentId {
    let mut request = admission("unused");
    request.agent_id = AgentId(format!("agt_inventory_{index:03}"));
    request.creation_key = format!("manual:{}", request.agent_id);
    request.origin = origin;
    let id = request.agent_id.clone();
    store.admit_agent(request).await.unwrap();
    id
}

#[tokio::test]
async fn provenance_inventory_filters_legacy_evidence_and_pages_past_excluded_candidates() {
    let store = Arc::new(ready_store(3).await);
    for index in 0..202 {
        let id = create(&store, index, AgentOrigin::Manual).await;
        store.lock().agents.get_mut(&id).unwrap().status = AgentStatus::Deleted;
    }
    let unknown = AgentId("agt_inventory_201".into());
    {
        let mut state = store.lock();
        state.agent_creation_provenance.clear();
        state.agent_logs.remove(&unknown);
    }
    let before = store.lock().clone();
    let service = service(store.clone());
    let query = AgentProvenanceInventoryQuery {
        creation_origin: Some(AgentCreationCategory::LegacyUnknown),
        ..Default::default()
    };
    let first = service
        .list_personal(&owner(), &query, &PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next.as_deref(), Some("agt_inventory_199"));
    let second = service
        .list_personal(&owner(), &query, &PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].agent_id, unknown);
    assert!(second.next.is_none());
    let manual = AgentProvenanceInventoryQuery {
        creation_origin: Some(AgentCreationCategory::Manual),
        ..Default::default()
    };
    let first = service
        .list_personal(&owner(), &manual, &PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.next.as_deref(), Some("agt_inventory_001"));
    assert_eq!(first.items.len(), 2);
    let second = service
        .list_personal(&owner(), &manual, &PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items[0].agent_id.as_str(), "agt_inventory_002");
    let after = store.lock();
    assert_eq!(before.agents, after.agents);
    assert_eq!(before.agent_logs, after.agent_logs);
    assert_eq!(
        before.agent_creation_provenance,
        after.agent_creation_provenance
    );
    assert_eq!(before.resource_policies, after.resource_policies);
}

#[tokio::test]
async fn provenance_inventory_keeps_retained_deleted_agents_and_refuses_corrupt_lookahead() {
    let store = Arc::new(ready_store(3).await);
    let first = create(&store, 0, AgentOrigin::Manual).await;
    let second = create(&store, 1, AgentOrigin::Manual).await;
    store.lock().agents.get_mut(&first).unwrap().status = AgentStatus::Deleted;
    let service = service(store.clone());
    let query = AgentProvenanceInventoryQuery::default();
    let page = service
        .list_personal(&owner(), &query, &PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items[0].status, AgentStatus::Deleted);
    assert_eq!(page.next.as_deref(), Some(first.as_str()));
    store
        .lock()
        .agent_creation_provenance
        .get_mut(&second)
        .unwrap()
        .agent_created_at = "corrupt".into();
    assert!(matches!(
        service
            .list_personal(&owner(), &query, &PageRequest::new(1, None).unwrap())
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        ))
    ));
}

#[tokio::test]
async fn provenance_inventory_personal_scope_excludes_explicit_org_even_with_same_subject() {
    let store = Arc::new(ready_store(3).await);
    create(&store, 0, AgentOrigin::Manual).await;
    let root = store.lock().sandboxes.values().next().unwrap().clone();
    store.lock().resource_policies.insert(
        root.id.clone(),
        ResourcePolicyRecord {
            sandbox_id: root.id,
            sandbox_created_at: root.created_at,
            owner: ResourceOwner::Organization {
                subject: root.owner.clone(),
            },
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "https://identity.test".into(),
                organization: root.owner,
                admitted_by: ResourceActor::Human {
                    subject: SubjectId::new("admin"),
                },
                verified_at: NOW.into(),
            }),
            revision: 1,
        },
    );
    assert!(store
        .scan_agent_provenance_inventory(
            &ResourceOwner::Person {
                subject: owner().subject
            },
            &Default::default(),
            &PageRequest::default()
        )
        .await
        .unwrap()
        .is_empty());
    assert!(service(store)
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery::default(),
            &PageRequest::default()
        )
        .await
        .unwrap()
        .items
        .is_empty());
}

use super::*;
use futures_util::TryStreamExt;
use layer2_core::resource_access::*;
use layer2_core::sandbox::{
    store::{AgentProvenanceInventoryStore, AgentProvenanceStore, PageRequest},
    AgentCreationCategory, AgentListQuery, AgentProvenanceInventoryQuery,
    AgentProvenanceInventoryService, AgentStatus,
};
use layer2_core::{Authorization, Caller, Capability};

struct NoAuthority;
#[async_trait::async_trait]
impl OrganizationAuthority for NoAuthority {
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

pub(super) fn service(store: Arc<MongoSandboxStore>) -> Arc<AgentProvenanceInventoryService> {
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoAuthority),
    ));
    AgentProvenanceInventoryService::new(store, access)
}
pub(super) fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn ids(rows: &[layer2_core::sandbox::store::AgentChannelUsageSnapshot]) -> Vec<&str> {
    rows.iter()
        .map(|row| {
            row.provenance
                .resource
                .lineage
                .agent
                .as_ref()
                .unwrap()
                .id
                .as_str()
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_inventory_reopens_and_advances_empty_filtered_pages_without_mutation() {
    let (client, database, store, _, workspace, _) = fixture("inventory_pages").await;
    for index in 0..202 {
        let mut request = admission(&workspace, &format!("inventory_{index:03}"));
        request.agent_id = AgentId(format!("agt_inventory_{index:03}"));
        request.creation_key = format!("manual:{}", request.agent_id);
        let mut agent = store
            .admit_agent(request)
            .await
            .unwrap()
            .creation()
            .agent
            .clone();
        let revision = agent.revision;
        agent.revision += 1;
        agent.status = AgentStatus::Deleted;
        agent.deleted_at = Some(NOW.into());
        store.save_agent(revision, agent).await.unwrap();
    }
    store
        .agent_creation_provenance
        .delete_many(doc! {})
        .await
        .unwrap();
    // Missing evidence remains unknown and must never cause writes during inventory.
    store
        .agent_log
        .delete_many(doc! {"agent_id":"agt_inventory_201"})
        .await
        .unwrap();
    let agents: Vec<Document> = store
        .agents
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let logs: Vec<Document> = store
        .agent_log
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let scope = ResourceOwner::Person {
        subject: owner().subject,
    };
    let scan = reopened
        .scan_agent_provenance_inventory(
            &scope,
            &AgentListQuery::default(),
            &PageRequest::new(50, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scan.len(), 51);
    assert_eq!(ids(&scan)[0], "agt_inventory_000");
    let service = service(reopened.clone());
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
    let last = service
        .list_personal(&owner(), &query, &PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.items[0].agent_id.as_str(), "agt_inventory_201");
    assert_eq!(last.items[0].status, AgentStatus::Deleted);
    assert!(last.next.is_none());
    let manual = service
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery {
                creation_origin: Some(AgentCreationCategory::Manual),
                ..Default::default()
            },
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manual.items.len(), 1);
    assert_eq!(manual.next.as_deref(), Some("agt_inventory_000"));
    assert_eq!(
        store
            .agents
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        agents
    );
    assert_eq!(
        store
            .agent_log
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        logs
    );
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_inventory_refuses_corrupt_lookahead_but_can_resume_after_it() {
    let (client, database, store, _, workspace, mut first) = fixture("inventory_corrupt").await;
    first.agent_id = AgentId("agt_inventory_a".into());
    store.admit_agent(first.clone()).await.unwrap();
    let mut second = admission(&workspace, "inventory_b");
    second.agent_id = AgentId("agt_inventory_b".into());
    store.admit_agent(second.clone()).await.unwrap();
    let retained = store
        .agent_provenance_snapshot(&first.agent_id)
        .await
        .unwrap();
    store
        .agent_creation_provenance
        .update_one(
            doc! {"_id":second.agent_id.as_str()},
            doc! {"$set":{"record.agent_created_at":"corrupt-private-timestamp"}},
        )
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let service = service(reopened);
    assert!(matches!(
        service
            .list_personal(
                &owner(),
                &AgentProvenanceInventoryQuery::default(),
                &PageRequest::new(1, None).unwrap()
            )
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        ))
    ));
    let after = service
        .list_personal(
            &owner(),
            &AgentProvenanceInventoryQuery::default(),
            &PageRequest::new(1, Some(second.agent_id.to_string())).unwrap(),
        )
        .await
        .unwrap();
    assert!(after.items.is_empty());
    assert!(after.next.is_none());
    assert_eq!(
        store
            .agent_provenance_snapshot(&first.agent_id)
            .await
            .unwrap(),
        retained
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_inventory_explicit_org_owner_excludes_same_subject_personal_scan() {
    use crate::store::sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION};
    let (client, database, store, root, _, request) = fixture("inventory_owner").await;
    let policy = ResourcePolicyRecord {
        sandbox_id: root.id.clone(),
        sandbox_created_at: root.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: root.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: root.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("fixture-admin"),
            },
            verified_at: root.created_at,
        }),
    };
    store.resource_policies.insert_one(doc! {"_id":root.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "owner_kind":"organization","owner_subject":root.owner.as_str(),"revision":1_i64,"record":record_bson(&policy).unwrap()}).await.unwrap();
    store.admit_agent(request.clone()).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let personal = ResourceOwner::Person {
        subject: root.owner.clone(),
    };
    let org = ResourceOwner::Organization {
        subject: root.owner,
    };
    let query = AgentListQuery::default();
    let page = PageRequest::new(1, None).unwrap();
    assert!(reopened
        .scan_agent_provenance_inventory(&personal, &query, &page)
        .await
        .unwrap()
        .is_empty());
    let result = reopened
        .scan_agent_provenance_inventory(&org, &query, &page)
        .await
        .unwrap();
    assert_eq!(ids(&result), [request.agent_id.as_str()]);
    assert_eq!(
        result[0]
            .provenance
            .creation_provenance(&request.agent_id)
            .unwrap()
            .owner,
        org
    );
    store
        .resource_policies
        .update_one(
            doc! {"_id":root.id.as_str()},
            doc! {"$set":{"record.sandbox_created_at":"stale-incarnation"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        reopened
            .scan_agent_provenance_inventory(&org, &query, &page)
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&database).drop().await.unwrap();
}

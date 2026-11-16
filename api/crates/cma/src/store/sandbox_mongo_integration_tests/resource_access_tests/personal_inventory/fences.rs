use super::*;
use layer2_core::sandbox::OwnerSandboxQuery;
use public::request;

struct Change {
    store: Arc<MongoSandboxStore>,
    collection: &'static str,
    update: Option<Document>,
}
#[async_trait::async_trait]
impl ResourceAccessStore for Change {
    async fn personal_agent_page_fence(
        &self,
        agents: &[layer2_core::sandbox::Agent],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessStoreError> {
        let collection = match self.collection {
            "agents" => &self.store.agents,
            "worktrees" => &self.store.worktrees,
            "workspaces" => &self.store.workspaces,
            "sandboxes" => &self.store.sandboxes,
            "resource_policies" => &self.store.resource_policies,
            "resource_shares" => &self.store.resource_shares,
            _ => unreachable!(),
        };
        if let Some(update) = &self.update {
            collection
                .update_one(doc! {}, update.clone())
                .await
                .unwrap();
        } else {
            collection.delete_one(doc! {}).await.unwrap();
        }
        self.store.personal_agent_page_fence(agents).await
    }
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("unexpected organization query")
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("unexpected shared query")
    }
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("unexpected organization query")
    }
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        panic!("batch failure must not fall back to per-row authorization")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_checks_fresh_lineage_and_revocation_before_release() {
    let f = fixture::Fixture::new(2, true).await;
    let target = ResourceTarget::Agent {
        agent_id: AgentId("agt_list_0000".into()),
    };
    let snapshot = f.store.resource_access_snapshot(&target).await.unwrap();
    f.store
        .mutate_share(mutation(&snapshot, "fence", ShareMode::View))
        .await
        .unwrap();
    for (collection, update) in [
        ("agents", None),
        ("worktrees", None),
        ("workspaces", None),
        ("sandboxes", None),
        (
            "agents",
            Some(doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}}),
        ),
        (
            "worktrees",
            Some(doc! {"$set":{"record.owner":"different-owner","owner":"different-owner"}}),
        ),
    ] {
        let records = f
            .client
            .database(&f.name)
            .collection::<Document>(collection);
        let original = records.find_one(doc! {}).await.unwrap().unwrap();
        let app = f.app_with_access(
            f.store.clone(),
            Arc::new(Change {
                store: f.store.clone(),
                collection,
                update,
            }),
            false,
        );
        let result = request(&app, "/api/v1/agents").await;
        assert!(
            !result.0.is_success(),
            "released changed {collection}: {result:?}"
        );
        assert!(result.1.get("items").is_none());
        records
            .replace_one(doc! {"_id":original.get("_id").unwrap().clone()}, original)
            .upsert(true)
            .await
            .unwrap();
    }
    assert!(request(&f.app(true, false), "/api/v1/agents")
        .await
        .0
        .is_success());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_never_interprets_an_organization_as_its_personal_owner() {
    let f = fixture::Fixture::new(1, false).await;
    let policy = ResourcePolicyRecord {
        sandbox_id: SandboxId("sbx_list_0000".into()),
        sandbox_created_at: NOW.into(),
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("owner"),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.example.test".into(),
            organization: SubjectId::new("owner"),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("human"),
            },
            verified_at: NOW.into(),
        }),
        revision: 1,
    };
    f.store
        .resource_policies
        .insert_one(crate::store::resource_access::records::policy_document(&policy).unwrap())
        .await
        .unwrap();
    let old = request(&f.app(false, false), "/api/v1/agents").await;
    assert_eq!(old.0, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(request(&f.app(true, false), "/api/v1/agents").await, old);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_keeps_tenant_and_revoked_browser_denials() {
    let f = fixture::Fixture::new(2, true).await;
    let old = f.app(false, false);
    let new = f.app(true, false);
    let path = "/api/v1/agents";
    let expected = request(&old, path).await;
    assert_eq!(expected.0, axum::http::StatusCode::OK);
    assert_eq!(request(&new, path).await, expected);
    f.identity.add_reader(1).await;
    let unrelated = public::request_cookie(&old, path, &identity::cookie(1)).await;
    assert_eq!(unrelated.0, axum::http::StatusCode::OK);
    assert_eq!(unrelated.1["items"].as_array().unwrap().len(), 0);
    assert_eq!(
        public::request_cookie(&new, path, &identity::cookie(1)).await,
        unrelated
    );
    f.identity.revoke(0).await;
    f.meter.take();
    let denied = request(&old, path).await;
    assert!(!denied.0.is_success());
    assert!(denied.1.get("items").is_none());
    assert_eq!(request(&new, path).await, denied);
    assert!(!f
        .meter
        .take()
        .commands
        .keys()
        .any(|key| key.starts_with("agents/")));
    f.close().await;
}

#[async_trait::async_trait]
impl ResourceShareStore for Change {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("list must not read mutation receipts")
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("list must not mutate shares")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("list must not issue per-share reads")
    }
}

use super::*;
use layer2_core::sandbox::OwnerSandboxQuery;
struct Change {
    store: Arc<MongoSandboxStore>,
    collection: &'static str,
    id: String,
    update: Option<Document>,
    revoke: Option<ShareMutation>,
    expiry: Option<Arc<Mutex<Option<tokio::time::Instant>>>>,
}
#[async_trait::async_trait]
impl ResourceAccessStore for Change {
    async fn organization_agent_page_fence(
        &self,
        agents: &[Agent],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessStoreError> {
        if let Some(revoke) = &self.revoke {
            self.store.mutate_share(revoke.clone()).await.unwrap();
        } else if let Some(expiry) = &self.expiry {
            let deadline = expiry.lock().unwrap().expect("membership observed");
            tokio::time::sleep_until(deadline + std::time::Duration::from_millis(1)).await;
        } else {
            let records = self
                .store
                .client
                .database(&self.store.agents.namespace().db)
                .collection::<Document>(self.collection);
            if let Some(update) = &self.update {
                records
                    .update_one(doc! {"_id":&self.id}, update.clone())
                    .await
                    .unwrap();
            } else {
                records.delete_one(doc! {"_id":&self.id}).await.unwrap();
            }
        }
        self.store.organization_agent_page_fence(agents).await
    }
    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store
            .organization_agents(organization, query, page)
            .await
    }
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("unexpected Sandbox inventory")
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("unexpected shared inventory")
    }
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        panic!("final fence must not fall back to row reads")
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for Change {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("read-only inventory")
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("read-only inventory")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("read-only inventory")
    }
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn organization_final_fence_refuses_changed_or_missing_lineage_including_lookahead() {
    let f = owned(201, true).await;
    for (collection, id, update) in [
        ("agents", "agt_list_0200", None),
        ("worktrees", "wkt_list_0200", None),
        ("workspaces", "wks_list_0200", None),
        ("sandboxes", "sbx_list_0200", None),
        (
            "resource_policies",
            "sbx_list_0200",
            Some(doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}}),
        ),
        (
            "agents",
            "agt_list_0200",
            Some(doc! {"$set":{"record.owner":"foreign","owner":"foreign"}}),
        ),
    ] {
        let records = f
            .client
            .database(&f.name)
            .collection::<Document>(collection);
        let original = records.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        let app = f.app_with_authority(
            f.store.clone(),
            Arc::new(Change {
                store: f.store.clone(),
                collection,
                id: id.into(),
                update,
                revoke: None,
                expiry: None,
            }),
            false,
            f.identity.auth(false),
        );
        let result = request(&app, PATH).await;
        assert!(
            !result.0.is_success(),
            "released changed {collection}: {result:?}"
        );
        assert!(result.1.get("items").is_none());
        records
            .replace_one(doc! {"_id":id}, original)
            .upsert(true)
            .await
            .unwrap();
    }
    assert_eq!(request(&app(&f, true), PATH).await.0, StatusCode::OK);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn organization_final_fence_refuses_revoked_share_and_plural_reads_preserve_order() {
    let f = fixture::Fixture::new(2, false).await;
    f.identity.add_reader(1).await;
    let root = ResourceTarget::Sandbox {
        sandbox_id: SandboxId("sbx_list_0000".into()),
    };
    let snapshot = f.store.resource_access_snapshot(&root).await.unwrap();
    let share = f
        .store
        .mutate_share(mutation(&snapshot, "org-fence", ShareMode::View))
        .await
        .unwrap()
        .share;
    let path = "/api/v1/agents?organization=org&limit=1";
    let expected = request(&app(&f, false), path).await;
    assert_eq!(expected.0, StatusCode::OK, "{expected:?}");
    assert_eq!(request(&app(&f, true), path).await, expected);
    let targets =
        ["agt_list_0001", "agt_list_0000", "agt_list_0001"].map(|id| ResourceTarget::Agent {
            agent_id: AgentId(id.into()),
        });
    assert_eq!(
        f.store.resource_access_snapshots(&targets).await.unwrap(),
        f.legacy.resource_access_snapshots(&targets).await.unwrap()
    );
    let missing = [ResourceTarget::Agent {
        agent_id: AgentId("missing".into()),
    }];
    assert_eq!(
        f.store.resource_access_snapshots(&missing).await,
        f.legacy.resource_access_snapshots(&missing).await
    );
    let current = f.store.resource_access_snapshot(&root).await.unwrap();
    let mut revoke = mutation(&current, "org-fence-revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke { share_id: share.id };
    revoke.expected_share_revision = Some(share.revision);
    let changed = f.app_with_authority(
        f.store.clone(),
        Arc::new(Change {
            store: f.store.clone(),
            collection: "",
            id: String::new(),
            update: None,
            revoke: Some(revoke),
            expiry: None,
        }),
        false,
        f.identity.auth(false),
    );
    let result = request(&changed, path).await;
    assert!(!result.0.is_success());
    assert!(result.1.get("items").is_none());
    assert!(request(&app(&f, true), path).await.1["items"]
        .as_array()
        .unwrap()
        .is_empty());
    f.close().await;
}
struct Expiring {
    inner: Arc<crate::auth::AuthState>,
    expiry: Arc<Mutex<Option<tokio::time::Instant>>>,
}
#[async_trait::async_trait]
impl OrganizationAuthority for Expiring {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut context = self
            .inner
            .organization_context(caller, credential, organization)
            .await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
        context.memberships[0].bound_monotonic_deadline(deadline);
        *self.expiry.lock().unwrap() = Some(deadline);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("presentation choices cannot authorize")
    }
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn organization_membership_expiry_during_final_fence_never_releases_a_page() {
    let f = owned(2, false).await;
    assert_eq!(request(&app(&f, true), PATH).await.0, StatusCode::OK);
    let expiry = Arc::new(Mutex::new(None));
    let changed = f.app_with_authority(
        f.store.clone(),
        Arc::new(Change {
            store: f.store.clone(),
            collection: "",
            id: String::new(),
            update: None,
            revoke: None,
            expiry: Some(expiry.clone()),
        }),
        false,
        Arc::new(Expiring {
            inner: f.identity.auth(false),
            expiry,
        }),
    );
    let result = request(&changed, PATH).await;
    assert!(
        !result.0.is_success(),
        "released expired membership: {result:?}"
    );
    assert!(result.1.get("items").is_none());
    f.close().await;
}

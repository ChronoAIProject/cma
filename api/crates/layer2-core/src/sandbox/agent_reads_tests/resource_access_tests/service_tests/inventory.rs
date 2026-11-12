use super::*;

fn query() -> AgentListQuery {
    AgentListQuery::default()
}
fn page(limit: u16, cursor: Option<&str>) -> PageRequest {
    PageRequest::new(limit, cursor.map(str::to_owned)).unwrap()
}
fn ids(page: &crate::sandbox::store::Page<AuthorizedResource>) -> Vec<&str> {
    page.items
        .iter()
        .map(|value| value.snapshot.lineage.agent.as_ref().unwrap().id.as_str())
        .collect()
}
fn selector(id: &str) -> ResourceSelector {
    ResourceSelector::Agent {
        agent_id: AgentId(id.into()),
    }
}

#[tokio::test]
async fn inventory_filters_before_pagination_and_deduplicates_overlapping_grants() {
    let (store, _, access, target) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    for id in ["a-private", "b-other-org", "z-shared"] {
        insert_hierarchy(
            &store,
            id,
            &format!("tree-{id}"),
            "workspace",
            "sandbox",
            "owner",
            AgentStatus::Starting,
        )
        .await;
    }
    let before = store
        .resource_access_snapshot(&target.target())
        .await
        .unwrap()
        .lineage;
    for (id, org) in [
        ("agent", "org"),
        ("b-other-org", "org-two"),
        ("z-shared", "org"),
    ] {
        let mut command = create(&selector(id), id);
        command.change = ShareChange::Create {
            target_org_id: SubjectId::new(org),
            mode: ShareMode::View,
        };
        access
            .change_share(&context("owner"), Some("request-credential"), command)
            .await
            .unwrap();
    }
    // Overlap must neither duplicate this Agent nor consume the lookahead slot.
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&target, "overlap"),
        )
        .await
        .unwrap();
    let first = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query(),
            &page(1, None),
        )
        .await
        .unwrap();
    assert_eq!(ids(&first), ["agent"]);
    assert_eq!(first.next.as_deref(), Some("agent"));
    let last = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query(),
            &page(1, first.next.as_deref()),
        )
        .await
        .unwrap();
    assert_eq!(ids(&last), ["z-shared"]);
    assert!(last.next.is_none());
    let other = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org-two"),
            &query(),
            &page(200, None),
        )
        .await
        .unwrap();
    assert_eq!(ids(&other), ["b-other-org"]);
    let absent = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("no-grants"),
            &query(),
            &page(200, None),
        )
        .await
        .unwrap();
    assert!(absent.items.is_empty());
    assert!(absent.next.is_none());
    let whole = ResourceSelector::Sandbox {
        sandbox_id: SandboxId("sandbox".into()),
    };
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&whole, "whole"),
        )
        .await
        .unwrap();
    let all = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query(),
            &page(200, None),
        )
        .await
        .unwrap();
    assert_eq!(ids(&all), ["a-private", "agent", "b-other-org", "z-shared"]);
    assert_eq!(
        store
            .resource_access_snapshot(&target.target())
            .await
            .unwrap()
            .lineage,
        before
    );
}

#[tokio::test]
async fn organization_scope_requires_explicit_ownership_and_fresh_request_evidence() {
    let (store, authority, access, _) = setup(Reply::Role(OrganizationRole::Member)).await;
    insert_hierarchy(
        &store,
        "org-agent",
        "org-tree",
        "org-workspace",
        "org-sandbox",
        "org",
        AgentStatus::Starting,
    )
    .await;
    let organization = SubjectId::new("org");
    assert!(access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &organization,
            &query(),
            &page(200, None)
        )
        .await
        .unwrap()
        .items
        .is_empty());
    store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: SandboxId("org-sandbox".into()),
        sandbox_created_at: NOW.into(),
        owner: ResourceOwner::Organization {
            subject: organization.clone(),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: organization.clone(),
            admitted_by: context("admin").actor,
            verified_at: NOW.into(),
        }),
        revision: 1,
    });
    let found = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &organization,
            &query(),
            &page(200, None),
        )
        .await
        .unwrap();
    assert_eq!(ids(&found), ["org-agent"]);
    let mut stale = context("recipient");
    stale.memberships = org_admin().memberships;
    for reply in [Reply::Missing, Reply::Outage, Reply::Foreign] {
        *authority.reply.lock().unwrap() = reply;
        assert!(access
            .organization_agent_inventory(
                &stale,
                Some("request-credential"),
                &organization,
                &query(),
                &page(200, None)
            )
            .await
            .is_err());
    }
    assert!(access
        .organization_agent_inventory(
            &context("recipient"),
            None,
            &organization,
            &query(),
            &page(200, None)
        )
        .await
        .is_err());
    // Personal inventory retains availability and performs no organization IO.
    let calls = authority.calls.load(Ordering::SeqCst);
    let reads = service(store).with_resource_access(Arc::new(access));
    assert_eq!(
        reads
            .list(&context("owner").caller, &query(), &page(200, None))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn parent_filters_cannot_disclose_ancestry_from_a_narrow_agent_share() {
    let (store, _, access, target) = setup(Reply::Role(OrganizationRole::Admin)).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&target, "grant"),
        )
        .await
        .unwrap();
    for id in ["sandbox", "absent-parent"] {
        let query = AgentListQuery {
            sandbox_id: Some(SandboxId(id.into())),
            ..Default::default()
        };
        assert!(matches!(
            access
                .organization_agent_inventory(
                    &context("recipient"),
                    Some("request-credential"),
                    &SubjectId::new("org"),
                    &query,
                    &page(200, None)
                )
                .await,
            Err(ResourceAccessError::Access(AccessError::NotFound))
                | Err(ResourceAccessError::Store(
                    ResourceAccessStoreError::NotFound
                ))
        ));
    }
    let query = AgentListQuery {
        workspace_id: Some(WorkspaceId("workspace".into())),
        ..Default::default()
    };
    assert!(access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query,
            &page(200, None)
        )
        .await
        .is_err());
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(
                &ResourceSelector::Sandbox {
                    sandbox_id: SandboxId("sandbox".into()),
                },
                "parent",
            ),
        )
        .await
        .unwrap();
    let list = access
        .organization_agent_inventory(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query,
            &page(200, None),
        )
        .await
        .unwrap();
    assert_eq!(ids(&list), ["agent"]);
    let reads = service(store).with_resource_access(Arc::new(access));
    let list = reads
        .organization_list_authorized(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &query,
            &page(200, None),
        )
        .await
        .unwrap();
    assert!(matches!(list.items[0], AgentDetailReading::Shared(_)));
}

struct GatedInventory {
    store: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    batch_reads: Arc<AtomicUsize>,
}
#[async_trait]
impl ResourceAccessStore for GatedInventory {
    async fn organization_sandboxes(
        &self,
        organization: &crate::SubjectId,
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store.organization_sandboxes(organization, query).await
    }

    async fn shared_sandboxes(
        &self,
        recipient: &crate::SubjectId,
        organizations: &[crate::SubjectId],
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.store
            .shared_sandboxes(recipient, organizations, query)
            .await
    }

    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        let result = self
            .store
            .organization_agents(organization, query, page)
            .await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.store.resource_access_snapshot(target).await
    }
    async fn resource_access_snapshots(
        &self,
        targets: &[ResourceTarget],
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.batch_reads.fetch_add(1, Ordering::SeqCst);
        self.store.resource_access_snapshots(targets).await
    }
}
#[async_trait]
impl ResourceShareStore for GatedInventory {
    async fn share_mutation_receipt(
        &self,
        id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.store.share_mutation_receipt(id).await
    }
    async fn mutate_share(
        &self,
        mutation: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.store.mutate_share(mutation).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.store.resource_share(id).await
    }
}

#[tokio::test]
async fn revoking_only_the_hidden_lookahead_rejects_a_buffered_next_cursor() {
    let (store, authority, access, target) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    insert_hierarchy(
        &store,
        "z-lookahead",
        "z-tree",
        "z-workspace",
        "z-sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&target, "first"),
        )
        .await
        .unwrap();
    let selector = selector("z-lookahead");
    let lookahead = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "last"),
        )
        .await
        .unwrap()
        .share;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let batch_reads = Arc::new(AtomicUsize::new(0));
    let gated = ResourceAccessService::new(
        Arc::new(GatedInventory {
            store,
            entered: entered.clone(),
            resume: resume.clone(),
            batch_reads: batch_reads.clone(),
        }),
        authority,
    );
    let task = tokio::spawn(async move {
        gated
            .organization_agent_inventory(
                &context("recipient"),
                Some("request-credential"),
                &SubjectId::new("org"),
                &query(),
                &page(1, None),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    access
        .change_share(&context("owner"), None, revoke(&selector, &lookahead))
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
    assert_eq!(batch_reads.load(Ordering::SeqCst), 1);
}

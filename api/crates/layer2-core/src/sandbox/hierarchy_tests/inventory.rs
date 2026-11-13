use super::*;

async fn list(
    f: &Fixture,
    org: &str,
    cursor: Option<&str>,
    limit: u16,
) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
    f.reads
        .organization_list_authorized(
            &context("recipient"),
            Some("request-credential"),
            &SubjectId::new(org),
            &OwnerSandboxQuery {
                exclude_retired: false,
                status: None,
                page: PageRequest::new(limit, cursor.map(str::to_owned)).unwrap(),
            },
        )
        .await
}

async fn extra(f: &Fixture, id: &str, shared: bool) -> Sandbox {
    let mut sandbox = f.parent.clone();
    sandbox.id = SandboxId(id.into());
    sandbox.name = id.into();
    insert_sandbox(&f.store, sandbox.clone()).await;
    if shared {
        f.access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: sandbox.id.clone(),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: format!("share-{id}"),
                },
            )
            .await
            .unwrap();
    }
    sandbox
}
fn id(value: &SandboxInventoryReading) -> &str {
    match value {
        SandboxInventoryReading::Personal(value) => value.sandbox.id.as_str(),
        SandboxInventoryReading::Shared(value) => value.id.as_str(),
    }
}

#[tokio::test]
async fn organization_sandbox_inventory_filters_before_paging_and_narrow_grants_never_count() {
    let f = fixture().await;
    grant(&f, false).await;
    extra(&f, "a_private", false).await;
    extra(&f, "z_shared", true).await;
    let first = list(&f, "org", None, 1).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(id(&first.items[0]), "z_shared");
    assert!(
        first.next.is_none(),
        "narrow-grant parent cannot become lookahead"
    );
    grant(&f, true).await;
    let first = list(&f, "org", None, 1).await.unwrap();
    assert_eq!(id(&first.items[0]), f.parent.id.as_str());
    assert_eq!(first.next.as_deref(), Some(f.parent.id.as_str()));
    let SandboxInventoryReading::Shared(summary) = &first.items[0] else {
        panic!("expected shared summary")
    };
    assert_eq!(
        summary.counts,
        HierarchyCounts {
            workspaces: 1,
            worktrees: 1,
            agents: 1
        }
    );
    let last = list(&f, "org", first.next.as_deref(), 1).await.unwrap();
    assert_eq!(id(&last.items[0]), "z_shared");
    assert!(last.next.is_none());
    assert!(list(&f, "other-org", None, 10)
        .await
        .unwrap()
        .items
        .is_empty());
    let page = f
        .reads
        .organization_list_authorized(
            &context("owner"),
            Some("request-credential"),
            &SubjectId::new("org"),
            &OwnerSandboxQuery::default(),
        )
        .await
        .unwrap();
    assert!(page
        .items
        .iter()
        .all(|item| matches!(item, SandboxInventoryReading::Personal(_))));
    let before = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    *f.authority.0.lock().unwrap() = Reply::Missing;
    assert!(matches!(
        list(&f, "org", None, 10).await,
        Err(HierarchyReadError::Access(ResourceAccessError::Membership(
            MembershipError::NotMember
        )))
    ));
    *f.authority.0.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        list(&f, "org", None, 10).await,
        Err(HierarchyReadError::Access(ResourceAccessError::Membership(
            MembershipError::Unavailable
        )))
    ));
    assert!(f
        .reads
        .list_sandboxes(&caller("owner", true), &OwnerSandboxQuery::default())
        .await
        .is_ok());
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap().unwrap(),
        before
    );
}

#[tokio::test]
async fn organization_sandbox_inventory_requires_explicit_owner_kind() {
    let f = fixture().await;
    assert!(list(&f, "owner", None, 10).await.unwrap().items.is_empty());
    let policy = ResourcePolicyRecord {
        sandbox_id: f.parent.id.clone(),
        sandbox_created_at: f.parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: f.parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: f.parent.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    f.store.set_resource_policy_for_test(policy);
    let page = list(&f, "owner", None, 10).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(matches!(page.items[0], SandboxInventoryReading::Shared(_)));
}

#[tokio::test]
async fn organization_inventory_rechecks_hidden_lookahead_after_buffered_count_io() {
    for expire in [false, true] {
        let f = fixture().await;
        grant(&f, true).await;
        let extra = extra(&f, "z_lookahead", true).await;
        if expire {
            *f.authority.0.lock().unwrap() = Reply::Expiring;
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let reads = HierarchyReadService::new(Arc::new(BufferedStore {
            projected: false,
            inner: f.store.clone(),
            sandbox: None,
            dependency_failure: false,
            gate: Some((entered.clone(), resume.clone())),
        }))
        .with_resource_access(f.access.clone());
        let task = tokio::spawn(async move {
            reads
                .organization_list_authorized(
                    &context("recipient"),
                    Some("request-credential"),
                    &SubjectId::new("org"),
                    &OwnerSandboxQuery {
                        exclude_retired: false,
                        status: None,
                        page: PageRequest::new(1, None).unwrap(),
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if expire {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(2)).await;
        } else {
            let snapshot = f
                .store
                .resource_access_snapshot(&ResourceTarget::Sandbox {
                    sandbox_id: extra.id,
                })
                .await
                .unwrap();
            let share = &snapshot.shares[0];
            f.access
                .change_share(
                    &context("owner"),
                    None,
                    ShareCommand {
                        selector: share.selector.clone(),
                        change: ShareChange::Revoke {
                            share_id: share.id.clone(),
                        },
                        expected_share_revision: Some(share.revision),
                        idempotency_key: "hidden-revoke".into(),
                    },
                )
                .await
                .unwrap();
        }
        resume.notify_one();
        let result = task.await.unwrap();
        if expire {
            tokio::time::resume();
            assert!(matches!(
                result,
                Err(HierarchyReadError::Access(ResourceAccessError::Access(
                    AccessError::RevalidationRequired
                )))
            ));
        } else {
            assert!(matches!(
                result,
                Err(HierarchyReadError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                )))
            ));
        }
    }
}

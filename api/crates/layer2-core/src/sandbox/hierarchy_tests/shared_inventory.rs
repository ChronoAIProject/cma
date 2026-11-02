use super::*;

async fn share(f: &Fixture, root: &Sandbox, org: &str, key: &str) -> ResourceShare {
    f.access
        .change_share(
            &context(root.owner.as_str()),
            Some("request-credential"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: root.id.clone(),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new(org),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: key.into(),
            },
        )
        .await
        .unwrap()
        .share
}
async fn list(
    f: &Fixture,
    recipient: &str,
    query: OwnerSandboxQuery,
) -> Result<Page<SandboxInventoryReading>, HierarchyReadError> {
    f.reads
        .shared_list_authorized(&context(recipient), Some("request-credential"), &query)
        .await
}
fn ids(page: &Page<SandboxInventoryReading>) -> Vec<&str> {
    page.items
        .iter()
        .map(|row| match row {
            SandboxInventoryReading::Shared(value) => value.id.as_str(),
            SandboxInventoryReading::Personal(_) => panic!("personal ownership must be excluded"),
        })
        .collect()
}

#[tokio::test]
async fn shared_sandbox_inventory_deduplicates_all_orgs_and_excludes_personal_and_narrow_roots_before_paging(
) {
    let f = fixture().await;
    grant(&f, false).await;
    let own = sandbox("a_own", "recipient", SandboxStatus::Active);
    let foreign = sandbox("z_shared", "other-owner", SandboxStatus::Active);
    for root in [&own, &foreign] {
        insert_sandbox(&f.store, root.clone()).await;
    }
    share(&f, &own, "org", "own").await;
    share(&f, &foreign, "org-two", "foreign").await;
    let query = OwnerSandboxQuery {
        exclude_retired: false,
        page: PageRequest::new(1, None).unwrap(),
        status: None,
    };
    let first = list(&f, "recipient", query.clone()).await.unwrap();
    assert_eq!(ids(&first), ["z_shared"]);
    assert_eq!(
        first.next, None,
        "narrow and own roots cannot become a lookahead"
    );
    grant(&f, true).await;
    share(&f, &f.parent, "org-two", "overlap").await;
    let first = list(&f, "recipient", query).await.unwrap();
    assert_eq!(ids(&first), ["sbx_parent"]);
    assert_eq!(first.next.as_deref(), Some("sbx_parent"));
    let last = list(
        &f,
        "recipient",
        OwnerSandboxQuery {
            exclude_retired: false,
            status: None,
            page: PageRequest::new(1, first.next).unwrap(),
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&last), ["z_shared"]);
    assert_eq!(last.next, None);
    assert!(list(
        &f,
        "recipient",
        OwnerSandboxQuery {
            status: Some(SandboxStatus::Paused),
            ..Default::default()
        }
    )
    .await
    .unwrap()
    .items
    .is_empty());
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(f.agent.clone())
    );
}

#[tokio::test]
async fn shared_sandbox_inventory_requires_explicit_grants_and_fresh_discovery_even_for_owners() {
    let f = fixture().await;
    f.store.set_resource_policy_for_test(ResourcePolicyRecord {
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
    });
    assert!(list(&f, "recipient", OwnerSandboxQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    *f.authority.0.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        list(&f, "owner", OwnerSandboxQuery::default()).await,
        Err(HierarchyReadError::Access(ResourceAccessError::Membership(
            MembershipError::Unavailable
        )))
    ));
    *f.authority.0.lock().unwrap() = Reply::Missing;
    assert!(list(&f, "recipient", OwnerSandboxQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(f
        .reads
        .shared_list_authorized(&context("recipient"), None, &OwnerSandboxQuery::default())
        .await
        .is_err());
}

#[tokio::test]
async fn shared_sandbox_inventory_fences_hidden_grants_and_original_deadline_after_count_io() {
    for failure in ["revoke", "expire", "status", "retired"] {
        let f = fixture().await;
        grant(&f, true).await;
        let mut extra = sandbox("z_lookahead", "owner", SandboxStatus::Active);
        insert_sandbox(&f.store, extra.clone()).await;
        let hidden = share(&f, &extra, "org-two", "hidden").await;
        if failure == "expire" {
            *f.authority.0.lock().unwrap() = Reply::Expiring;
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let reads = HierarchyReadService::new(Arc::new(BufferedStore {
            inner: f.store.clone(),
            sandbox: None,
            dependency_failure: false,
            gate: Some((entered.clone(), resume.clone())),
        }))
        .with_resource_access(f.access.clone());
        let task = tokio::spawn(async move {
            reads
                .shared_list_authorized(
                    &context("recipient"),
                    Some("request-credential"),
                    &OwnerSandboxQuery {
                        exclude_retired: failure == "retired",
                        status: (failure != "retired").then_some(SandboxStatus::Active),
                        page: PageRequest::new(1, None).unwrap(),
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if failure == "expire" {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(2)).await;
        } else if failure == "status" || failure == "retired" {
            extra.status = if failure == "retired" {
                SandboxStatus::Retired
            } else {
                SandboxStatus::Paused
            };
            extra.revision += 1;
            f.store.save_sandbox(1, extra).await.unwrap();
        } else {
            f.access
                .change_share(
                    &context("owner"),
                    None,
                    ShareCommand {
                        selector: hidden.selector,
                        change: ShareChange::Revoke {
                            share_id: hidden.id,
                        },
                        expected_share_revision: Some(hidden.revision),
                        idempotency_key: "revoke-hidden".into(),
                    },
                )
                .await
                .unwrap();
        }
        resume.notify_one();
        let result = task.await.unwrap();
        if failure == "expire" {
            tokio::time::resume();
        }
        assert!(
            result.is_err(),
            "{failure} must not expose even a continuation cursor"
        );
        assert_eq!(
            f.store.get_agent(&f.agent.id).await.unwrap(),
            Some(f.agent.clone())
        );
    }
}

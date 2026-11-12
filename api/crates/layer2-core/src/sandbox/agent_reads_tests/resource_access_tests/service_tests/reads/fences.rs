use super::*;
#[path = "fences/owner.rs"]
mod owner;

struct PolicyStore {
    inner: Arc<InMemorySandboxStore>,
    replacement: Option<ResourceAccessSnapshot>,
    calls: AtomicUsize,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
#[async_trait]
impl ResourceAccessStore for PolicyStore {
    async fn organization_sandboxes(
        &self,
        organization: &crate::SubjectId,
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(organization, query).await
    }

    async fn shared_sandboxes(
        &self,
        recipient: &crate::SubjectId,
        organizations: &[crate::SubjectId],
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(recipient, organizations, query)
            .await
    }

    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .organization_agents(organization, query, page)
            .await
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 3 {
            if let Some((entered, resume)) = &self.gate {
                entered.notify_one();
                resume.notified().await;
            }
        }
        match &self.replacement {
            Some(snapshot) => Ok(snapshot.clone()),
            None => self.inner.resource_access_snapshot(target).await,
        }
    }
}
#[async_trait]
impl ResourceShareStore for PolicyStore {
    async fn share_mutation_receipt(
        &self,
        scope: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(scope).await
    }
    async fn mutate_share(
        &self,
        command: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.inner.mutate_share(command).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
}

#[tokio::test]
async fn explicit_org_owner_never_becomes_personal_on_legacy_interfaces() {
    let (store, authority, _, target) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let mut snapshot = store
        .resource_access_snapshot(&target.target())
        .await
        .unwrap();
    snapshot.policy = Some(ResourcePolicyRecord {
        sandbox_id: snapshot.lineage.sandbox.id.clone(),
        sandbox_created_at: snapshot.lineage.sandbox.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("owner"),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: SubjectId::new("owner"),
            admitted_by: context("admin").actor,
            verified_at: NOW.into(),
        }),
    });
    let access = Arc::new(ResourceAccessService::new(
        Arc::new(PolicyStore {
            inner: store.clone(),
            replacement: Some(snapshot),
            calls: AtomicUsize::new(0),
            gate: None,
        }),
        authority,
    ));
    let reads = service(store).with_resource_access(access);
    let id = AgentId("agent".into());
    assert!(matches!(
        reads.lineage(&context("owner").caller, &id).await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads.log_snapshot(&context("owner").caller, &id).await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads
            .list(
                &context("owner").caller,
                &AgentListQuery::default(),
                &PageRequest::new(200, None).unwrap()
            )
            .await,
        Err(AgentReadError::NotFound)
    ));
    assert!(matches!(
        reads
            .detail_authorized(&context("member"), Some("request-credential"), &id)
            .await
            .unwrap(),
        AgentDetailReading::Shared(_)
    ));
}

#[tokio::test]
async fn grant_revocation_during_buffered_history_rejects_the_pending_page() {
    for projection in ["events", "transcript", "transcript_page", "usage"] {
        let (store, authority, access, selector) =
            setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let share = access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap()
            .share;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let policy_store = Arc::new(PolicyStore {
            inner: store.clone(),
            replacement: None,
            calls: AtomicUsize::new(0),
            gate: Some((entered.clone(), resume.clone())),
        });
        let reads = service(store.clone()).with_resource_access(Arc::new(
            ResourceAccessService::new(policy_store, authority),
        ));
        let task = tokio::spawn(async move {
            match projection {
                "events" => reads
                    .log_authorized(
                        &context("recipient"),
                        Some("request-credential"),
                        &agent.id,
                        0,
                        2,
                    )
                    .await
                    .map(|_| ()),
                "transcript_page" => reads
                    .transcript_page_authorized(
                        &context("recipient"),
                        Some("request-credential"),
                        &agent.id,
                        &crate::sandbox::AgentTranscriptPageRequest {
                            turn: None,
                            after: None,
                            through: None,
                            limit: 2,
                        },
                    )
                    .await
                    .map(|_| ()),
                "transcript" => reads
                    .transcript_authorized(
                        &context("recipient"),
                        Some("request-credential"),
                        &agent.id,
                    )
                    .await
                    .map(|_| ()),
                _ => reads
                    .usage_authorized(&context("recipient"), Some("request-credential"), &agent.id)
                    .await
                    .map(|_| ()),
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        access
            .change_share(&context("owner"), None, revoke(&selector, &share))
            .await
            .unwrap();
        resume.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            Err(AgentReadError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            )))
        ));
    }
}

struct GatedResources {
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl SandboxResourceReader for GatedResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(SandboxResourceReading::default())
    }
}

#[tokio::test]
async fn sandbox_grant_revocation_during_metrics_io_rejects_buffered_counters() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let selector = ResourceSelector::Sandbox {
        sandbox_id: SandboxId("sandbox".into()),
    };
    let share = access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let reads = AgentReadService::new(
        store,
        Arc::new(GatedResources {
            entered: entered.clone(),
            resume: resume.clone(),
        }),
    )
    .with_resource_access(Arc::new(access.clone()));
    let task = tokio::spawn(async move {
        reads
            .resources_authorized(
                &context("recipient"),
                Some("request-credential"),
                &AgentId("agent".into()),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    access
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(AgentReadError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        )))
    ));
}

#[tokio::test]
async fn buffered_result_cannot_extend_expired_membership() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let snapshot = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let mut recipient = context("recipient");
    let now = SystemTime::now();
    recipient.memberships.push(
        OrganizationMembership::new(
            recipient.credential.clone(),
            SubjectId::new("org"),
            OrganizationRole::Viewer,
            now,
            now + Duration::from_millis(20),
        )
        .unwrap(),
    );
    let decision = AccessDecision::evaluate(
        &recipient,
        &snapshot.scope(&selector.target()).unwrap(),
        &snapshot.shares,
        AccessPurpose::Read,
        now,
    )
    .unwrap();
    decision.require(ResourceCapability::ViewHistory).unwrap();
    let held = AuthorizedResource {
        snapshot,
        decision,
        caller: recipient.caller.clone(),
        binding: None,
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        access
            .validate_read_result(&held, &selector.target(), ResourceCapability::ViewHistory)
            .await,
        Err(ResourceAccessError::Access(
            AccessError::RevalidationRequired
        ))
    );
}

#[tokio::test]
async fn held_narrow_authorization_cannot_validate_a_sibling_result() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    insert_hierarchy(
        &store,
        "sibling",
        "sibling-tree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let held = access
        .authorize(
            &context("recipient"),
            Some("request-credential"),
            &selector.target(),
            ResourceCapability::ViewHistory,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    assert!(access
        .validate_read_result(
            &held,
            &ResourceTarget::Agent {
                agent_id: AgentId("sibling".into())
            },
            ResourceCapability::ViewHistory
        )
        .await
        .is_err());
}

#[tokio::test]
async fn stream_policy_io_is_bounded_and_cannot_release_after_revocation_or_expiry() {
    for fault in ["revoked", "expired", "timeout"] {
        let (store, authority, access, selector) =
            setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let share = access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap()
            .share;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let reads =
            service(store.clone()).with_resource_access(Arc::new(ResourceAccessService::new(
                Arc::new(PolicyStore {
                    inner: store,
                    replacement: None,
                    calls: AtomicUsize::new(0),
                    gate: Some((entered.clone(), resume.clone())),
                }),
                authority,
            )));
        let held = reads
            .authorize_event_stream(&context("recipient"), Some("request-credential"), &agent.id)
            .await
            .unwrap();
        let task = tokio::spawn(async move { reads.event_stream_snapshot(&held).await });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if fault == "revoked" {
            access
                .change_share(&context("owner"), None, revoke(&selector, &share))
                .await
                .unwrap();
            resume.notify_one();
        } else {
            tokio::time::pause();
            tokio::time::advance(if fault == "expired" {
                MAX_MEMBERSHIP_AGE
            } else {
                Duration::from_secs(15)
            })
            .await;
        }
        let result = task.await.unwrap();
        if fault != "revoked" {
            tokio::time::resume();
        }
        match fault {
            "revoked" => assert!(matches!(
                result,
                Err(AgentReadError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                )))
            )),
            "expired" => assert!(matches!(
                result,
                Err(AgentReadError::Access(ResourceAccessError::Access(
                    AccessError::RevalidationRequired
                )))
            )),
            _ => assert!(matches!(result, Err(AgentReadError::Unavailable(_)))),
        }
    }
}

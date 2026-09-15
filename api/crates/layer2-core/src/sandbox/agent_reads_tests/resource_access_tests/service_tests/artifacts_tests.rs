use super::*;
use crate::sandbox::agent_artifacts::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn body(name: &str) -> PublishAgentArtifact {
    PublishAgentArtifact {
        name: name.into(),
        media_type: "text/plain".into(),
        content_base64: STANDARD.encode("approved report\n"),
    }
}
fn page() -> ArtifactPageRequest {
    ArtifactPageRequest {
        limit: 1,
        cursor: None,
    }
}

#[tokio::test]
async fn approved_artifacts_are_immutable_narrow_and_preserve_retained_resources() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let access = Arc::new(access);
    let service = AgentArtifactService::new(store.clone(), access.clone());
    let owner = context("owner");
    let recipient = context("recipient");
    let id = AgentId("agent".into());
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let logs = store.agent_log(&id, 0, 200).await.unwrap();
    let first = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "one",
            body("report.txt"),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "one",
                body("report.txt")
            )
            .await
            .unwrap(),
        first
    );
    assert!(matches!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "one",
                body("different.txt")
            )
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::IdempotencyConflict
        ))
    ));
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &first.id
        )
        .await
        .is_err());
    let share = access
        .change_share(
            &owner,
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let decision = access
        .authorize(
            &recipient,
            Some("request-credential"),
            &selector.target(),
            ResourceCapability::ViewArtifacts,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    assert!(!decision.decision.allows(ResourceCapability::ViewFiles));
    assert!(matches!(
        service
            .publish(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                "recipient",
                body("intrusion.txt")
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::Forbidden))
    ));
    let second = service
        .publish(
            &owner.caller,
            None,
            None,
            &id,
            "legacy",
            body("older caller.txt"),
        )
        .await
        .unwrap();
    let one = service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &page(),
        )
        .await
        .unwrap();
    assert_eq!(one.items.len(), 1);
    let two = service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &ArtifactPageRequest {
                cursor: one.next.clone(),
                ..page()
            },
        )
        .await
        .unwrap();
    assert_eq!(two.items.len(), 1);
    assert_ne!(one.items, two.items);
    assert!(two.next.is_none());
    assert_eq!(
        service
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                &first.id
            )
            .await
            .unwrap()
            .1,
        b"approved report\n"
    );
    assert!(service
        .read(&recipient.caller, None, None, &id, &first.id)
        .await
        .is_err());
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
    let sibling_id = AgentId("sibling".into());
    let sibling = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &sibling_id,
            "sibling",
            body("sibling.txt"),
        )
        .await
        .unwrap();
    assert!(service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &sibling_id,
            &page()
        )
        .await
        .is_err());
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &sibling_id,
            &sibling.id
        )
        .await
        .is_err());
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &sibling.id
        )
        .await
        .is_err());
    let stored = store
        .agent_artifact(&ArtifactBinding::from_snapshot(&before).unwrap(), &first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.authorization.actor, owner.actor);
    assert_eq!(
        stored.authorization.owner,
        ResourceOwner::Person {
            subject: owner.caller.subject.clone()
        }
    );
    *authority.reply.lock().unwrap() = Reply::Missing;
    assert!(service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &page()
        )
        .await
        .is_err());
    *authority.reply.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        service
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                &first.id
            )
            .await,
        Err(ResourceAccessError::Membership(
            MembershipError::Unavailable
        ))
    ));
    access
        .change_share(&owner, None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &second.id
        )
        .await
        .is_err());
    assert_eq!(
        service
            .read(&owner.caller, None, None, &id, &first.id)
            .await
            .unwrap()
            .0,
        first
    );
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    assert_eq!(store.agent_log(&id, 0, 200).await.unwrap(), logs);
}

struct BufferedArtifacts {
    store: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    gate_publish: bool,
    gate_read: bool,
}
#[async_trait::async_trait]
impl AgentArtifactStore for BufferedArtifacts {
    async fn publish_artifact(
        &self,
        request: &ArtifactPublication,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        if self.gate_publish {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.store.publish_artifact(request).await
    }
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifact>, ResourceAccessStoreError> {
        let result = self.store.agent_artifacts(binding, page).await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError> {
        let result = self.store.agent_artifact(binding, id).await?;
        if self.gate_read {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(result)
    }
}

#[tokio::test]
async fn artifacts_refuse_revocation_after_content_io_and_policy_changes_before_publication() {
    for read_list in [false, true] {
        let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
        let access = Arc::new(access);
        let owner = context("owner");
        let id = AgentId("agent".into());
        let original = AgentArtifactService::new(store.clone(), access.clone())
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "one",
                body("report.txt"),
            )
            .await
            .unwrap();
        let share = access
            .change_share(
                &owner,
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap()
            .share;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let service = AgentArtifactService::new(
            Arc::new(BufferedArtifacts {
                store: store.clone(),
                entered: entered.clone(),
                resume: resume.clone(),
                gate_publish: false,
                gate_read: true,
            }),
            access.clone(),
        );
        let task = tokio::spawn(async move {
            let recipient = context("recipient");
            if read_list {
                service
                    .list(
                        &recipient.caller,
                        Some(&recipient),
                        Some("request-credential"),
                        &id,
                        &page(),
                    )
                    .await
                    .map(|_| ())
            } else {
                service
                    .read(
                        &recipient.caller,
                        Some(&recipient),
                        Some("request-credential"),
                        &id,
                        &original.id,
                    )
                    .await
                    .map(|_| ())
            }
        });
        entered.notified().await;
        access
            .change_share(&owner, None, revoke(&selector, &share))
            .await
            .unwrap();
        resume.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            Err(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            ))
        ));
    }
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let access = Arc::new(access);
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let service = AgentArtifactService::new(
        Arc::new(BufferedArtifacts {
            store: store.clone(),
            entered: entered.clone(),
            resume: resume.clone(),
            gate_publish: true,
            gate_read: false,
        }),
        access.clone(),
    );
    let task = tokio::spawn(async move {
        let owner = context("owner");
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &AgentId("agent".into()),
                "one",
                body("report.txt"),
            )
            .await
    });
    entered.notified().await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "new-scope"),
        )
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
    let snapshot = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    assert!(store
        .agent_artifacts(&ArtifactBinding::from_snapshot(&snapshot).unwrap(), &page())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test(start_paused = true)]
async fn artifact_reads_do_not_release_bytes_after_the_request_deadline() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let service = AgentArtifactService::new(
        Arc::new(BufferedArtifacts {
            store,
            entered: entered.clone(),
            resume: Arc::new(tokio::sync::Notify::new()),
            gate_publish: false,
            gate_read: true,
        }),
        Arc::new(access),
    );
    let task = tokio::spawn(async move {
        let owner = context("owner");
        service
            .list(&owner.caller, None, None, &AgentId("agent".into()), &page())
            .await
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Unavailable
        ))
    ));
}

#[test]
fn artifact_inputs_cannot_select_urls_paths_or_oversized_content() {
    for name in [
        "../report",
        "/home/cma/file",
        "secret\\file",
        ".",
        "..",
        "bad\nname",
        " trailing ",
    ] {
        assert!(body(name).content().is_err());
    }
    let mut value = body("report.txt");
    value.media_type = "text/html".into();
    assert!(value.content().is_err());
    value = body("report.txt");
    value.content_base64 = "https://private.test".into();
    assert!(value.content().is_err());
    value.content_base64 = STANDARD.encode(vec![0; MAX_ARTIFACT_BYTES + 1]);
    assert!(value.content().is_err());
    value.content_base64 = STANDARD.encode(vec![0; MAX_ARTIFACT_BYTES]);
    assert!(value.content().is_ok());
}

#[tokio::test]
async fn organization_publication_keeps_human_actor_and_refuses_members_and_delegated_senders() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    let original = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let owner = SubjectId::new("owner");
    store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: original.lineage.sandbox.id.clone(),
        sandbox_created_at: original.lineage.sandbox.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: owner.clone(),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: owner.clone(),
            admitted_by: context("admin").actor,
            verified_at: crate::now_rfc3339(),
        }),
        revision: 1,
    });
    let service = AgentArtifactService::new(store.clone(), Arc::new(access));
    let admin = context("admin");
    let id = AgentId("agent".into());
    let metadata = service
        .publish(
            &admin.caller,
            Some(&admin),
            Some("request-credential"),
            &id,
            "org-report",
            body("org.txt"),
        )
        .await
        .unwrap();
    let snapshot = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let stored = store
        .agent_artifact(
            &ArtifactBinding::from_snapshot(&snapshot).unwrap(),
            &metadata.id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.authorization.actor, admin.actor);
    assert_eq!(
        stored.authorization.owner,
        ResourceOwner::Organization { subject: owner }
    );
    *authority.reply.lock().unwrap() = Reply::Role(OrganizationRole::Viewer);
    assert!(service
        .read(
            &admin.caller,
            Some(&admin),
            Some("request-credential"),
            &id,
            &metadata.id
        )
        .await
        .is_ok());
    assert!(matches!(
        service
            .publish(
                &admin.caller,
                Some(&admin),
                Some("request-credential"),
                &id,
                "viewer",
                body("no.txt")
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::Forbidden))
    ));
    assert!(service
        .read(&context("owner").caller, None, None, &id, &metadata.id)
        .await
        .is_err());
    *authority.reply.lock().unwrap() = Reply::Role(OrganizationRole::Admin);
    let mut delegated = admin.clone();
    delegated.actor = ResourceActor::Api {
        subject: admin.caller.subject.clone(),
        delegation_id: Some("external-sender".into()),
    };
    assert!(service
        .publish(
            &delegated.caller,
            Some(&delegated),
            Some("request-credential"),
            &id,
            "delegated",
            body("no.txt")
        )
        .await
        .is_err());
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap()
            .lineage,
        original.lineage
    );
}

#[tokio::test]
async fn artifact_capacity_bounds_new_content_while_preserving_idempotent_recovery() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let service = AgentArtifactService::new(store, Arc::new(access));
    let owner = context("owner");
    let id = AgentId("agent".into());
    let first = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "0",
            body("report.txt"),
        )
        .await
        .unwrap();
    for number in 1..MAX_AGENT_ARTIFACTS {
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                &number.to_string(),
                body("report.txt"),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "overflow",
                body("report.txt")
            )
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Capacity
        ))
    ));
    assert_eq!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "0",
                body("report.txt")
            )
            .await
            .unwrap(),
        first
    );
}

struct ExpiringAuthority;
#[async_trait::async_trait]
impl OrganizationAuthority for ExpiringAuthority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        let now = SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Viewer,
            now,
            now + Duration::from_secs(1),
        )?);
        Ok(current)
    }
}

#[tokio::test(start_paused = true)]
async fn artifact_download_keeps_original_membership_deadline_after_buffering_bytes() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let owner = context("owner");
    let id = AgentId("agent".into());
    let original = AgentArtifactService::new(store.clone(), Arc::new(access.clone()))
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "one",
            body("report.txt"),
        )
        .await
        .unwrap();
    access
        .change_share(
            &owner,
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let service = AgentArtifactService::new(
        Arc::new(BufferedArtifacts {
            store: store.clone(),
            entered: entered.clone(),
            resume: resume.clone(),
            gate_publish: false,
            gate_read: true,
        }),
        Arc::new(ResourceAccessService::new(
            store,
            Arc::new(ExpiringAuthority),
        )),
    );
    let task = tokio::spawn(async move {
        let recipient = context("recipient");
        service
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                &original.id,
            )
            .await
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    resume.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(ResourceAccessError::Access(
            AccessError::RevalidationRequired
        ))
    ));
}

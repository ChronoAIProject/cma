use super::*;

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

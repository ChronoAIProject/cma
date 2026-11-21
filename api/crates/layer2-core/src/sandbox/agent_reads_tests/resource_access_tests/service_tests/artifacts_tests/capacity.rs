use super::*;

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

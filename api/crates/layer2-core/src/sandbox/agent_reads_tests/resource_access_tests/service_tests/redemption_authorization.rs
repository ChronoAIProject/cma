use super::*;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

async fn link(
    service: &ResourceAccessService,
    selector: &ResourceSelector,
    audience: AgentShareAudience,
) -> ResourceShare {
    service
        .change_share(
            &context("owner"),
            Some("request-credential"),
            ShareCommand {
                selector: selector.clone(),
                change: ShareChange::CreateLink {
                    audience,
                    mode: ShareMode::View,
                    token_sha256: format!("{:x}", Sha256::digest(TOKEN.as_bytes())),
                },
                expected_share_revision: None,
                idempotency_key: "routing-link".into(),
            },
        )
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn redemption_target_resolution_is_read_only_and_needs_no_existing_agent_access() {
    let (store, _, service, selector) = setup(Reply::Missing).await;
    let share = link(&service, &selector, AgentShareAudience::Public).await;
    let recipient = context("recipient");
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let decision = AccessDecision::evaluate(
        &recipient,
        &before.scope(&selector.target()).unwrap(),
        &before.shares,
        AccessPurpose::Read,
        SystemTime::now(),
    )
    .unwrap();
    assert!(!decision.allows(ResourceCapability::ViewDetails));
    assert!(!decision.allows(ResourceCapability::ManageSharing));
    for _ in 0..2 {
        assert_eq!(
            service
                .authorize_agent_share_redemption(&recipient, None, &share.id, TOKEN)
                .await
                .unwrap(),
            AgentId("agent".into()),
        );
        assert_eq!(
            store
                .resource_access_snapshot(&selector.target())
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            store.resource_share(&share.id).await.unwrap(),
            Some(share.clone())
        );
    }
    let redeemed = service
        .redeem_agent_share_link(&recipient, None, &share.id, TOKEN)
        .await
        .unwrap();
    assert_eq!(redeemed.revision, share.revision + 1);
    assert_eq!(
        redeemed.link.as_ref().unwrap().accepted_subjects.as_slice(),
        std::slice::from_ref(&recipient.caller.subject)
    );
    let after = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    assert_eq!(
        service
            .authorize_agent_share_redemption(&recipient, None, &share.id, TOKEN)
            .await
            .unwrap(),
        AgentId("agent".into())
    );
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn redemption_target_resolution_preserves_token_principal_audience_and_revocation_refusals() {
    let (store, _, service, selector) = setup(Reply::Missing).await;
    let share = link(
        &service,
        &selector,
        AgentShareAudience::Users {
            subjects: vec![SubjectId::new("recipient")],
        },
    )
    .await;
    let recipient = context("recipient");
    let mut forged = recipient.clone();
    forged.credential.subject = SubjectId::new("someone-else");
    let mut no_capability = recipient.clone();
    no_capability.caller.authorization = Authorization::none();
    let mut wrong_actor = recipient.clone();
    wrong_actor.actor = ResourceActor::Human {
        subject: SubjectId::new("someone-else"),
    };
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    for caller in [context("outsider"), forged, no_capability, wrong_actor] {
        assert_eq!(
            service
                .authorize_agent_share_redemption(&caller, None, &share.id, TOKEN)
                .await,
            Err(AccessError::NotFound.into())
        );
    }
    for (id, token) in [
        (share.id.as_str(), "b".repeat(64)),
        ("missing", TOKEN.to_owned()),
    ] {
        assert_eq!(
            service
                .authorize_agent_share_redemption(&recipient, None, id, &token)
                .await,
            Err(AccessError::NotFound.into())
        );
    }
    assert_eq!(
        service
            .authorize_agent_share_redemption(&recipient, None, &share.id, "invalid")
            .await,
        Err(ResourceAccessStoreError::Invalid.into())
    );
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        before
    );
    service
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    let revoked = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    assert_eq!(
        service
            .authorize_agent_share_redemption(&recipient, None, &share.id, TOKEN)
            .await,
        Err(AccessError::NotFound.into())
    );
    assert_eq!(
        service
            .redeem_agent_share_link(&recipient, None, &share.id, TOKEN)
            .await,
        Err(AccessError::NotFound.into())
    );
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        revoked
    );
}

#[tokio::test]
async fn redemption_target_resolution_requires_fresh_exact_organization_evidence() {
    let (store, authority, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
    let share = link(
        &service,
        &selector,
        AgentShareAudience::Organization {
            organization_id: SubjectId::new("org"),
        },
    )
    .await;
    let recipient = context("recipient");
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    assert_eq!(
        service
            .authorize_agent_share_redemption(&recipient, None, &share.id, TOKEN)
            .await,
        Err(MembershipError::Unavailable.into())
    );
    for (reply, error) in [
        (Reply::Missing, MembershipError::NotMember),
        (Reply::Outage, MembershipError::Unavailable),
        (Reply::Foreign, MembershipError::InvalidEvidence),
        (Reply::Stale, MembershipError::InvalidEvidence),
    ] {
        *authority.reply.lock().unwrap() = reply;
        assert_eq!(
            service
                .authorize_agent_share_redemption(
                    &recipient,
                    Some("request-credential"),
                    &share.id,
                    TOKEN
                )
                .await,
            Err(error.clone().into())
        );
        assert_eq!(
            service
                .redeem_agent_share_link(&recipient, Some("request-credential"), &share.id, TOKEN)
                .await,
            Err(error.into())
        );
    }
    *authority.reply.lock().unwrap() = Reply::Role(OrganizationRole::Viewer);
    let calls = authority.calls.load(Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(
            service
                .authorize_agent_share_redemption(
                    &recipient,
                    Some("request-credential"),
                    &share.id,
                    TOKEN
                )
                .await
                .unwrap(),
            AgentId("agent".into())
        );
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), calls + 2);
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn redemption_target_resolution_refuses_revocation_during_membership_io() {
    let (store, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
    let share = link(
        &service,
        &selector,
        AgentShareAudience::Organization {
            organization_id: SubjectId::new("org"),
        },
    )
    .await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let gated = ResourceAccessService::new(
        store.clone(),
        Arc::new(Authority {
            reply: Mutex::new(Reply::Role(OrganizationRole::Viewer)),
            calls: AtomicUsize::new(0),
            gate: Some((entered.clone(), resume.clone())),
        }),
    );
    let id = share.id.clone();
    let pending = tokio::spawn(async move {
        gated
            .authorize_agent_share_redemption(
                &context("recipient"),
                Some("request-credential"),
                &id,
                TOKEN,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    service
        .change_share(&context("owner"), None, revoke(&selector, &share))
        .await
        .unwrap();
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    resume.notify_one();
    assert_eq!(pending.await.unwrap(), Err(AccessError::NotFound.into()));
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap(),
        before
    );
}

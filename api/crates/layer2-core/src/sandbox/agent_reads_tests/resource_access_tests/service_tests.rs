#[path = "service_tests/artifacts_tests.rs"]
mod artifacts;
#[path = "service_tests/inventory.rs"]
mod inventory;
#[path = "service_tests/reads.rs"]
mod reads;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime};

use super::*;

#[derive(Clone)]
enum Reply {
    Role(OrganizationRole),
    Missing,
    Outage,
    Foreign,
    Stale,
}
struct Authority {
    reply: Mutex<Reply>,
    calls: AtomicUsize,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: caller(subject, true),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "request".into(),
        },
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        memberships: Vec::new(),
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "request-credential");
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, resume)) = &self.gate {
            entered.notify_one();
            resume.notified().await;
        }
        let reply = self.reply.lock().unwrap().clone();
        let (role, age) = match reply {
            Reply::Role(role) => (role, Duration::ZERO),
            Reply::Missing => return Err(MembershipError::NotMember),
            Reply::Outage => return Err(MembershipError::Unavailable),
            Reply::Foreign => (OrganizationRole::Admin, Duration::ZERO),
            Reply::Stale => (OrganizationRole::Admin, Duration::from_secs(1)),
        };
        let mut evidence = context(caller.subject.as_str());
        if matches!(reply, Reply::Foreign) {
            evidence.credential.issuer = "https://other.test".into();
        }
        let now = SystemTime::now() - age;
        evidence.memberships.push(
            OrganizationMembership::new(
                evidence.credential.clone(),
                organization.clone(),
                role,
                now,
                now + MAX_MEMBERSHIP_AGE,
            )
            .unwrap(),
        );
        Ok(evidence)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!("presentation inventory cannot authorize shares")
    }
}
async fn setup(
    reply: Reply,
) -> (
    Arc<InMemorySandboxStore>,
    Arc<Authority>,
    ResourceAccessService,
    ResourceSelector,
) {
    let (store, _) = fixture().await;
    let store = Arc::new(store);
    let authority = Arc::new(Authority {
        reply: Mutex::new(reply),
        calls: AtomicUsize::new(0),
        gate: None,
    });
    let service = ResourceAccessService::new(store.clone(), authority.clone());
    (
        store,
        authority,
        service,
        ResourceSelector::Agent {
            agent_id: AgentId("agent".into()),
        },
    )
}
fn create(selector: &ResourceSelector, key: &str) -> ShareCommand {
    ShareCommand {
        selector: selector.clone(),
        change: ShareChange::Create {
            target_org_id: SubjectId::new("org"),
            mode: ShareMode::View,
        },
        expected_share_revision: None,
        idempotency_key: key.into(),
    }
}
fn revoke(selector: &ResourceSelector, share: &ResourceShare) -> ShareCommand {
    ShareCommand {
        selector: selector.clone(),
        change: ShareChange::Revoke {
            share_id: share.id.clone(),
        },
        expected_share_revision: Some(share.revision),
        idempotency_key: "revoke".into(),
    }
}

#[tokio::test]
async fn ownership_stays_available_through_org_outage_and_retries_do_not_regrant() {
    let (_, authority, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
    let owner = context("owner");
    let receipt = service
        .change_share(
            &owner,
            Some("request-credential"),
            create(&selector, "create"),
        )
        .await
        .unwrap();
    assert_eq!(receipt.actor, owner.actor);
    assert_eq!(receipt.share.issuer_actor, owner.actor);
    *authority.reply.lock().unwrap() = Reply::Outage;
    let revoked = service
        .change_share(&owner, None, revoke(&selector, &receipt.share))
        .await
        .unwrap();
    assert!(revoked.share.revoked_at.is_some());
    assert_eq!(
        service
            .change_share(&owner, None, create(&selector, "create"))
            .await
            .unwrap(),
        receipt
    );
    assert!(service
        .active_shares(&owner, None, &selector)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        service
            .read_share(&owner, None, &selector, &revoked.share.id)
            .await
            .unwrap(),
        revoked.share
    );
    assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
    assert!(service
        .authorize(
            &owner,
            None,
            &selector.target(),
            ResourceCapability::Interact,
            AccessPurpose::Read
        )
        .await
        .unwrap()
        .decision
        .allows(ResourceCapability::Interact));
}

#[tokio::test]
async fn recipients_cannot_reshare_or_read_parent_or_files_and_removal_blocks_reads() {
    let (_, authority, service, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    service
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "create"),
        )
        .await
        .unwrap();
    let recipient = context("recipient");
    let read = service
        .authorize(
            &recipient,
            Some("request-credential"),
            &selector.target(),
            ResourceCapability::ViewHistory,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    assert!(!read.decision.allows(ResourceCapability::ViewFiles));
    assert!(!read.decision.allows(ResourceCapability::Interact));
    assert_eq!(
        service
            .change_share(
                &recipient,
                Some("request-credential"),
                create(&selector, "reshare")
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::Forbidden))
    );
    assert!(matches!(
        service
            .authorize(
                &recipient,
                Some("request-credential"),
                &ResourceTarget::Sandbox {
                    sandbox_id: SandboxId("sandbox".into())
                },
                ResourceCapability::ViewDetails,
                AccessPurpose::Read
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::NotFound))
    ));
    *authority.reply.lock().unwrap() = Reply::Missing;
    assert!(matches!(
        service
            .authorize(
                &recipient,
                Some("request-credential"),
                &selector.target(),
                ResourceCapability::ViewHistory,
                AccessPurpose::Read
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::NotFound))
    ));
}

#[tokio::test]
async fn forged_principals_and_foreign_or_stale_evidence_cannot_create_grants() {
    for reply in [Reply::Foreign, Reply::Stale, Reply::Outage] {
        let (store, _, service, selector) = setup(reply).await;
        let before = store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap();
        assert!(service
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "create")
            )
            .await
            .is_err());
        assert_eq!(
            store
                .resource_access_snapshot(&selector.target())
                .await
                .unwrap(),
            before
        );
    }
    let (_, authority, service, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    let mut forged = context("owner");
    forged.actor = ResourceActor::Human {
        subject: SubjectId::new("someone-else"),
    };
    assert_eq!(
        service
            .change_share(
                &forged,
                Some("request-credential"),
                create(&selector, "create")
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::NotFound))
    );
    assert_eq!(authority.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revocation_during_provider_io_invalidates_the_pending_read() {
    let (store, _, service, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    let owner = context("owner");
    let receipt = service
        .change_share(
            &owner,
            Some("request-credential"),
            create(&selector, "create"),
        )
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let gated = ResourceAccessService::new(
        store.clone(),
        Arc::new(Authority {
            reply: Mutex::new(Reply::Role(OrganizationRole::Admin)),
            calls: AtomicUsize::new(0),
            gate: Some((entered.clone(), resume.clone())),
        }),
    );
    let target = selector.target();
    let pending = tokio::spawn(async move {
        gated
            .authorize(
                &context("recipient"),
                Some("request-credential"),
                &target,
                ResourceCapability::ViewHistory,
                AccessPurpose::Read,
            )
            .await
    });
    entered.notified().await;
    service
        .change_share(&owner, None, revoke(&selector, &receipt.share))
        .await
        .unwrap();
    resume.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
}

#[tokio::test]
async fn stale_caller_memberships_never_authorize_a_new_read_or_mutation() {
    let (_, authority, service, selector) = setup(Reply::Role(OrganizationRole::Admin)).await;
    service
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "create"),
        )
        .await
        .unwrap();
    let mut recipient = context("recipient");
    let now = SystemTime::now();
    recipient.memberships.push(
        OrganizationMembership::new(
            recipient.credential.clone(),
            SubjectId::new("org"),
            OrganizationRole::Admin,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )
        .unwrap(),
    );
    *authority.reply.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        service
            .authorize(
                &recipient,
                Some("request-credential"),
                &selector.target(),
                ResourceCapability::ViewHistory,
                AccessPurpose::Read
            )
            .await,
        Err(ResourceAccessError::Membership(
            MembershipError::Unavailable
        ))
    ));
}

#[path = "service_tests/standing.rs"]
mod standing;

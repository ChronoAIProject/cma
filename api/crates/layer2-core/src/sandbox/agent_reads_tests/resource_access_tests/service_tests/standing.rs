use super::*;
use crate::account_deletion::{AccountDeletionError, AccountStanding};
struct Standing(&'static str);
#[async_trait::async_trait]
impl AccountStanding for Standing {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(subject.as_str() == self.0)
    }
}
#[tokio::test]
async fn standing_gate_denies_caller_and_shared_resource_owner() {
    for deleting in ["owner", "member"] {
        let (_, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
        service
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap();
        let service = service.with_account_standing(Arc::new(Standing(deleting)));
        let error = service
            .authorize(
                &context("member"),
                Some("request-credential"),
                &selector.target(),
                ResourceCapability::ViewHistory,
                AccessPurpose::Read,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error, ResourceAccessError::AccountDeletionInProgress);
    }
}
#[tokio::test]
async fn standing_other_owner_is_unaffected() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    let service = service.with_account_standing(Arc::new(Standing("unrelated")));
    assert!(service
        .authorize(
            &context("owner"),
            None,
            &selector.target(),
            ResourceCapability::ViewHistory,
            AccessPurpose::Read
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn standing_gate_primary_resource_keeps_deleting_owner_concealed() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    for target in [
        selector.target(),
        ResourceTarget::Sandbox {
            sandbox_id: SandboxId("sandbox".into()),
        },
    ] {
        assert_eq!(
            service
                .authorize(
                    &context("unrelated"),
                    None,
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read,
                )
                .await
                .err(),
            Some(ResourceAccessError::Access(AccessError::NotFound)),
        );
        let deleting = service
            .clone()
            .with_account_standing(Arc::new(Standing("owner")));
        assert_eq!(
            deleting
                .authorize(
                    &context("unrelated"),
                    None,
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read,
                )
                .await
                .err(),
            Some(ResourceAccessError::Access(AccessError::NotFound)),
        );
        assert_eq!(
            deleting
                .authorize(
                    &context("owner"),
                    None,
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read,
                )
                .await
                .err(),
            Some(ResourceAccessError::AccountDeletionInProgress),
        );
    }
}

#[tokio::test]
async fn standing_gate_personal_resource_keeps_deleting_owner_concealed() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    for target in [
        selector.target(),
        ResourceTarget::Sandbox {
            sandbox_id: SandboxId("sandbox".into()),
        },
    ] {
        assert_eq!(
            service
                .authorize_personal(
                    &context("unrelated").caller,
                    &target,
                    ResourceCapability::ViewHistory,
                )
                .await
                .err(),
            Some(ResourceAccessError::Access(AccessError::NotFound)),
        );
        let deleting = service
            .clone()
            .with_account_standing(Arc::new(Standing("owner")));
        assert_eq!(
            deleting
                .authorize_personal(
                    &context("unrelated").caller,
                    &target,
                    ResourceCapability::ViewHistory,
                )
                .await
                .err(),
            Some(ResourceAccessError::Access(AccessError::NotFound)),
        );
        assert_eq!(
            deleting
                .authorize_personal(
                    &context("owner").caller,
                    &target,
                    ResourceCapability::ViewHistory,
                )
                .await
                .err(),
            Some(ResourceAccessError::AccountDeletionInProgress),
        );
    }
}

#[tokio::test]
async fn resource_share_grant_to_erasing_recipient_is_refused() {
    let (_, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
    let service = service.with_account_standing(Arc::new(Standing("recipient")));
    let error = service
        .change_share(
            &context("owner"),
            Some("request-credential"),
            ShareCommand {
                selector: selector.clone(),
                change: ShareChange::CreateLink {
                    audience: AgentShareAudience::Users {
                        subjects: vec![SubjectId::new("recipient")],
                    },
                    mode: ShareMode::View,
                    token_sha256: "a".repeat(64),
                },
                expected_share_revision: None,
                idempotency_key: "grant".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        ResourceAccessStoreError::ShareRecipientUnavailable.into()
    );
    assert!(service
        .active_shares(&context("owner"), None, &selector)
        .await
        .unwrap()
        .is_empty());
}

#[derive(Default)]
struct ReadStanding {
    unavailable: Mutex<Option<&'static str>>,
    authoritative_calls: AtomicUsize,
}
#[async_trait::async_trait]
impl AccountStanding for ReadStanding {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(self
            .unavailable
            .lock()
            .unwrap()
            .is_some_and(|s| subject.as_str() == s))
    }
    async fn authoritative(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.authoritative_calls.fetch_add(1, Ordering::SeqCst);
        Err(AccountDeletionError::Unavailable)
    }
}

#[tokio::test]
async fn buffered_shared_read_rechecks_caller_and_owner_using_snapshot_standing() {
    for deleting in ["member", "owner", "unrelated"] {
        let (_, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
        service
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "grant"),
            )
            .await
            .unwrap();
        let standing = Arc::new(ReadStanding::default());
        let service = service.with_account_standing(standing.clone());
        let held = service
            .authorize(
                &context("member"),
                Some("request-credential"),
                &selector.target(),
                ResourceCapability::ViewHistory,
                AccessPurpose::Read,
            )
            .await
            .unwrap();
        assert!(service
            .validate_read_result(&held, &selector.target(), ResourceCapability::ViewHistory)
            .await
            .is_ok());
        *standing.unavailable.lock().unwrap() = Some(deleting);
        let result = service
            .validate_read_result(&held, &selector.target(), ResourceCapability::ViewHistory)
            .await;
        if deleting == "unrelated" {
            assert!(result.is_ok());
        } else {
            assert_eq!(result, Err(ResourceAccessError::AccountDeletionInProgress));
        }
        assert_eq!(standing.authoritative_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn organization_share_id_is_not_a_personal_deletion_recipient() {
    let (_, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
    let service = service.with_account_standing(Arc::new(Standing("org")));
    let receipt = service
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "org-grant"),
        )
        .await
        .unwrap();
    assert_eq!(receipt.share.target_org_id, SubjectId::new("org"));
    assert!(service
        .authorize(
            &context("member"),
            Some("request-credential"),
            &selector.target(),
            ResourceCapability::ViewHistory,
            AccessPurpose::Read
        )
        .await
        .is_ok());
}

async fn public_link(
    service: &ResourceAccessService,
    selector: &ResourceSelector,
) -> ResourceShare {
    use sha2::{Digest, Sha256};
    service
        .change_share(
            &context("owner"),
            None,
            ShareCommand {
                selector: selector.clone(),
                change: ShareChange::CreateLink {
                    audience: AgentShareAudience::Public,
                    mode: ShareMode::View,
                    token_sha256: format!("{:x}", Sha256::digest("a".repeat(64).as_bytes())),
                },
                expected_share_revision: None,
                idempotency_key: "public-link".into(),
            },
        )
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn invalid_link_token_does_not_query_authoritative_recipient_standing() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    let share = public_link(&service, &selector).await;
    let standing = Arc::new(ReadStanding::default());
    let service = service.with_account_standing(standing.clone());
    assert_eq!(
        service
            .redeem_agent_share_link(&context("recipient"), None, &share.id, "invalid")
            .await,
        Err(ResourceAccessStoreError::Invalid.into())
    );
    assert_eq!(standing.authoritative_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn valid_link_redemption_uses_transaction_recipient_fence_without_point_query() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    let share = public_link(&service, &selector).await;
    let standing = Arc::new(ReadStanding::default());
    let service = service.with_account_standing(standing.clone());
    let redeemed = service
        .redeem_agent_share_link(&context("recipient"), None, &share.id, &"a".repeat(64))
        .await
        .unwrap();
    assert_eq!(
        redeemed.link.unwrap().accepted_subjects,
        [SubjectId::new("recipient")]
    );
    assert_eq!(standing.authoritative_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn link_redemption_refuses_an_erasing_sharing_owner() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    let share = public_link(&service, &selector).await;
    let service = service.with_account_standing(Arc::new(Standing("owner")));
    assert_eq!(
        service
            .redeem_agent_share_link(&context("recipient"), None, &share.id, &"a".repeat(64))
            .await,
        Err(ResourceAccessError::AccountDeletionInProgress)
    );
}

#[tokio::test]
async fn well_formed_wrong_link_token_conceals_recipient_standing() {
    let (_, _, service, selector) = setup(Reply::Missing).await;
    let share = public_link(&service, &selector).await;
    let service = service.with_account_standing(Arc::new(Standing("recipient")));
    assert_eq!(
        service
            .redeem_agent_share_link(&context("recipient"), None, &share.id, &"b".repeat(64))
            .await,
        Err(ResourceAccessError::Access(AccessError::NotFound))
    );
}

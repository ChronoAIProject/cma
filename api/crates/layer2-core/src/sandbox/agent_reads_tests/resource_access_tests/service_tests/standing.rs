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
    for change in [
        ShareChange::Create {
            target_org_id: SubjectId::new("recipient"),
            mode: ShareMode::View,
        },
        ShareChange::CreateLink {
            audience: AgentShareAudience::Users {
                subjects: vec![SubjectId::new("recipient")],
            },
            mode: ShareMode::View,
            token_sha256: "a".repeat(64),
        },
    ] {
        let (_, _, service, selector) = setup(Reply::Role(OrganizationRole::Member)).await;
        let service = service.with_account_standing(Arc::new(Standing("recipient")));
        let error = service
            .change_share(
                &context("owner"),
                Some("request-credential"),
                ShareCommand {
                    selector: selector.clone(),
                    change,
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
}

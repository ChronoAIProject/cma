use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use layer2_core::{
    account_deletion::{
        AccountDeletion, AccountDeletionError, AccountDeletionId, AccountDeletionService,
        AccountDeletionStore, AccountStanding, AvailableAccounts, ErasureEvent,
        MemoryAccountDeletionStore, OwnerSessionAuthority,
    },
    Authorization, Capability, SubjectId,
};

use super::Scenario;

struct FaultStore {
    mode: &'static str,
    inner: MemoryAccountDeletionStore,
}

impl FaultStore {
    fn failure(&self) -> AccountDeletionError {
        match self.mode {
            "recovery" => AccountDeletionError::RecoveryRequired,
            _ => AccountDeletionError::Store("contract account deletion store unavailable".into()),
        }
    }
}

#[async_trait]
impl AccountDeletionStore for FaultStore {
    async fn request(
        &self,
        _: &SubjectId,
        _: &str,
        _: &str,
    ) -> Result<AccountDeletion, AccountDeletionError> {
        Err(self.failure())
    }

    async fn replay(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.replay(subject, key).await
    }

    async fn active(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.active(subject).await
    }

    async fn latest(&self, _: &SubjectId) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        Err(self.failure())
    }

    async fn get(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.get(id).await
    }

    async fn claim_due(
        &self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.inner.claim_due(holder, now, lease).await
    }

    async fn commit(
        &self,
        deletion: AccountDeletion,
        events: Vec<ErasureEvent>,
        expected_revision: u64,
        expected_lease_token: Option<&str>,
    ) -> Result<(), AccountDeletionError> {
        self.inner
            .commit(deletion, events, expected_revision, expected_lease_token)
            .await
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("account_deletion_")?;
    let store: Arc<dyn AccountDeletionStore> = match mode {
        "store_error" | "recovery" => Arc::new(FaultStore {
            mode: match mode {
                "recovery" => "recovery",
                _ => "store_error",
            },
            inner: MemoryAccountDeletionStore::default(),
        }),
        "owner" | "disabled" | "anonymous" | "nonhuman" => {
            Arc::new(MemoryAccountDeletionStore::default())
        }
        _ => panic!("unknown account deletion contract scenario {mode}"),
    };
    let router = super::super::account_deletion::account_deletion_router(Arc::new(
        AccountDeletionService::new(store, Arc::new(AvailableAccounts), mode != "disabled"),
    ));
    let router = if mode == "nonhuman" || mode == "anonymous" {
        router
    } else {
        router.layer(axum::Extension(OwnerSessionAuthority::verified_personal(
            SubjectId::new("owner"),
        )))
    };
    Some(Scenario {
        router,
        subject: (mode != "anonymous").then_some("owner"),
        authorization: if mode == "nonhuman" {
            Authorization::new([Capability::UseBindingConversations])
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}

/// Recipient-only refusal leaves the authenticated resource owner available.
pub(super) struct ErasingRecipient;
#[async_trait]
impl AccountStanding for ErasingRecipient {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(matches!(subject.as_str(), "recipient" | "document-reader"))
    }
}

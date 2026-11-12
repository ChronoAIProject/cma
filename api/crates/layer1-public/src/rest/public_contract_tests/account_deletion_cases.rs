use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use layer2_core::{
    account_deletion::{
        AccountDeletion, AccountDeletionError, AccountDeletionId, AccountDeletionService,
        AccountDeletionStore, AccountStanding, AvailableAccounts, ErasureEvent,
        MemoryAccountDeletionStore,
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
            "conflict" => AccountDeletionError::Conflict,
            "in_progress" => AccountDeletionError::InProgress,
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
    ) -> Result<(), AccountDeletionError> {
        self.inner.commit(deletion, events, expected_revision).await
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("account_deletion_")?;
    let store: Arc<dyn AccountDeletionStore> = match mode {
        "store_error" | "recovery" | "conflict" | "in_progress" => Arc::new(FaultStore {
            mode: match mode {
                "recovery" => "recovery",
                "conflict" => "conflict",
                "in_progress" => "in_progress",
                _ => "store_error",
            },
            inner: MemoryAccountDeletionStore::default(),
        }),
        "owner" | "disabled" | "anonymous" | "nonhuman" => {
            Arc::new(MemoryAccountDeletionStore::default())
        }
        _ => panic!("unknown account deletion contract scenario {mode}"),
    };
    Some(Scenario {
        router: super::super::account_deletion::account_deletion_router(Arc::new(
            AccountDeletionService::new(store, Arc::new(AvailableAccounts), mode != "disabled"),
        )),
        subject: (mode != "anonymous").then_some("owner"),
        authorization: if mode == "nonhuman" {
            Authorization::new([Capability::UseBindingConversations])
        } else {
            Authorization::new([Capability::DeleteOwnAccount])
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

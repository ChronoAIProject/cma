//! Owner and operator access to retained Sandbox allocation usage.

use crate::sandbox::store::{Page, PageRequest, SandboxStoreError};
use crate::sandbox_usage::{SandboxUsage, UsageEvent, UsageTotals};
use crate::{Caller, Capability, SandboxId, SubjectId};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummary {
    pub sandbox_count: u64,
    pub running_count: u64,
    pub incomplete_lifecycle_count: u64,
    pub tracking_started_at: Option<u64>,
    pub totals: UsageTotals,
}

#[async_trait]
pub trait SandboxUsageStore: Send + Sync {
    async fn usage(&self, id: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError>;
    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError>;
    async fn usage_summary(
        &self,
        owner: Option<&SubjectId>,
        as_of: u64,
    ) -> Result<UsageSummary, SandboxStoreError>;
    async fn usage_events(
        &self,
        id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum UsageReadError {
    #[error("usage access denied")]
    Forbidden,
    #[error("sandbox usage not found")]
    NotFound,
    #[error(transparent)]
    Store(#[from] SandboxStoreError),
}

pub struct SandboxUsageService {
    store: Arc<dyn SandboxUsageStore>,
}

impl SandboxUsageService {
    pub fn new(store: Arc<dyn SandboxUsageStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    pub async fn list(
        &self,
        caller: &Caller,
        admin: bool,
        owner: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<SandboxUsage>, UsageReadError> {
        let owner = scope(caller, admin, owner)?;
        Ok(self.store.usage_page(owner, page).await?)
    }

    pub async fn summary(
        &self,
        caller: &Caller,
        admin: bool,
        owner: Option<&SubjectId>,
        as_of: u64,
    ) -> Result<UsageSummary, UsageReadError> {
        let owner = scope(caller, admin, owner)?;
        Ok(self.store.usage_summary(owner, as_of).await?)
    }

    pub async fn read(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxUsage, UsageReadError> {
        let value = self
            .store
            .usage(id)
            .await?
            .ok_or(UsageReadError::NotFound)?;
        if caller.authorization.allows(Capability::ReadAllSandboxes)
            || (value.owner == caller.subject && can_read_own(caller))
        {
            Ok(value)
        } else {
            Err(UsageReadError::NotFound)
        }
    }

    pub async fn events(
        &self,
        caller: &Caller,
        id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, UsageReadError> {
        self.read(caller, id).await?;
        Ok(self.store.usage_events(id, page).await?)
    }
}

fn can_read_own(caller: &Caller) -> bool {
    caller.authorization.allows(Capability::UseOwnSandboxes)
        || caller.authorization.allows(Capability::LaunchTriggers)
}

fn scope<'a>(
    caller: &'a Caller,
    admin: bool,
    owner: Option<&'a SubjectId>,
) -> Result<Option<&'a SubjectId>, UsageReadError> {
    if admin {
        caller
            .authorization
            .allows(Capability::ReadAllSandboxes)
            .then_some(owner)
            .ok_or(UsageReadError::Forbidden)
    } else if can_read_own(caller) && owner.is_none_or(|owner| owner == &caller.subject) {
        Ok(Some(&caller.subject))
    } else {
        Err(UsageReadError::Forbidden)
    }
}

#[cfg(test)]
#[path = "sandbox_usage_service_tests.rs"]
mod tests;

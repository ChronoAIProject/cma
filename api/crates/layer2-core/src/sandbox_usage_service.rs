//! Owner and operator access to retained Sandbox allocation usage.

use crate::sandbox::store::{Page, PageRequest, SandboxStoreError};
use crate::sandbox_usage::{SandboxUsage, UsageEvent, UsageTotals};
use crate::sandbox_usage_cursor::{CursorError, Scope, UsageCursors};
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
    #[error("invalid usage continuation; restart from the first page")]
    InvalidCursor,
    #[error("usage access denied")]
    Forbidden,
    #[error("sandbox usage not found")]
    NotFound,
    #[error(transparent)]
    Store(#[from] SandboxStoreError),
}

pub struct SandboxUsageService {
    store: Arc<dyn SandboxUsageStore>,
    cursors: UsageCursors,
}

impl SandboxUsageService {
    pub fn new(store: Arc<dyn SandboxUsageStore>, cursors: UsageCursors) -> Arc<Self> {
        Arc::new(Self { store, cursors })
    }

    pub async fn list(
        &self,
        caller: &Caller,
        admin: bool,
        owner: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<SandboxUsage>, UsageReadError> {
        let owner = scope(caller, admin, owner)?;
        let scope = Scope {
            caller,
            admin,
            owner,
            sandbox: None,
            filter_owner: owner,
        };
        let chain = self.cursors.begin(&scope, page).map_err(cursor_error)?;
        let mut result = self.store.usage_page(owner, &chain.page).await?;
        result.next = self
            .cursors
            .finish(&scope, chain, result.next)
            .map_err(cursor_error)?;
        Ok(result)
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
        owner_filter: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, UsageReadError> {
        let retained = self.read(caller, id).await?;
        let scope = Scope {
            caller,
            admin: caller.authorization.allows(Capability::ReadAllSandboxes),
            owner: Some(&retained.owner),
            sandbox: Some(id),
            filter_owner: owner_filter,
        };
        let chain = self.cursors.begin(&scope, page).map_err(cursor_error)?;
        let mut result = self.store.usage_events(id, &chain.page).await?;
        result.next = self
            .cursors
            .finish(&scope, chain, result.next)
            .map_err(cursor_error)?;
        Ok(result)
    }
}

fn cursor_error(error: CursorError) -> UsageReadError {
    match error {
        CursorError::Invalid => UsageReadError::InvalidCursor,
        CursorError::Store(error) => UsageReadError::Store(error),
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

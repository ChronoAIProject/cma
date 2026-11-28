//! Captured ordinary-work authority shared by renewal and publication adapters.

use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use tokio::time::Instant;

use super::{SandboxStoreError, WorkLeaseGrant, WORK_LEASE_SECONDS, WORK_RENEW_SECONDS};
use crate::sandbox::{Sandbox, Workspace};

#[derive(Clone)]
pub enum ClaimedWork {
    Sandbox(Box<Sandbox>),
    Workspace(Box<Workspace>),
}

/// The identity is immutable, even when an operation loads a newer revision.
/// Nested Workspace execution also retains the originating Sandbox authority.
#[derive(Clone)]
pub struct WorkAuthority {
    claimed: ClaimedWork,
    parent: Option<Box<Sandbox>>,
    validity: Arc<Validity>,
}

struct Validity {
    until: Mutex<Instant>,
    closed: AtomicBool,
}

tokio::task_local! { static CURRENT: Vec<WorkAuthority>; }

impl WorkAuthority {
    pub fn new(claimed: ClaimedWork, request_start: Instant) -> Self {
        Self {
            claimed,
            parent: None,
            validity: Arc::new(Validity {
                until: Mutex::new(request_start + Duration::from_secs(WORK_LEASE_SECONDS)),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub fn with_parent(mut self, parent: Sandbox) -> Result<Self, SandboxStoreError> {
        if !matches!(&self.claimed, ClaimedWork::Workspace(row)
            if row.sandbox_id == parent.id && row.owner == parent.owner)
        {
            return Err(SandboxStoreError::Conflict);
        }
        self.parent = Some(Box::new(parent));
        Ok(self)
    }

    pub fn parent(&self) -> Option<&Sandbox> {
        self.parent.as_deref()
    }

    pub fn claimed(&self) -> &ClaimedWork {
        &self.claimed
    }

    pub fn deadline(&self) -> Result<Instant, SandboxStoreError> {
        let until = *self
            .validity
            .until
            .lock()
            .map_err(|_| SandboxStoreError::Conflict)?;
        if self.validity.closed.load(Ordering::Acquire) || Instant::now() >= until {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(until)
    }

    pub fn current() -> Result<Vec<Self>, SandboxStoreError> {
        let chain = CURRENT.try_with(Clone::clone).unwrap_or_default();
        for authority in &chain {
            authority.deadline()?;
        }
        Ok(chain)
    }

    pub fn active() -> bool {
        CURRENT.try_with(|chain| !chain.is_empty()).unwrap_or(false)
    }

    fn accept(&self, start: Instant, grant: WorkLeaseGrant) -> Result<(), SandboxStoreError> {
        self.deadline()?;
        let until = start + grant.valid_for;
        if grant.valid_for > Duration::from_secs(WORK_LEASE_SECONDS) || Instant::now() >= until {
            return Err(SandboxStoreError::Conflict);
        }
        *self
            .validity
            .until
            .lock()
            .map_err(|_| SandboxStoreError::Conflict)? = until;
        Ok(())
    }

    async fn heartbeat<R, F>(&self, renew: &mut R) -> Result<Instant, SandboxStoreError>
    where
        R: FnMut() -> F,
        F: Future<Output = Result<Option<WorkLeaseGrant>, SandboxStoreError>>,
    {
        let start = Instant::now();
        let limit = self
            .deadline()?
            .min(start + Duration::from_secs(WORK_RENEW_SECONDS));
        let grant = tokio::time::timeout_at(limit, renew())
            .await
            .map_err(|_| SandboxStoreError::Conflict)??
            .ok_or(SandboxStoreError::Conflict)?;
        self.accept(start, grant)?;
        Ok(start + Duration::from_secs(WORK_RENEW_SECONDS))
    }

    /// The effect future is never polled before an immediate renewal succeeds.
    /// A slow effect cannot suppress heartbeat polling. Dropping this scope
    /// cancels local work; it does not claim that remote effects were cancelled.
    pub async fn run<T, W, R, F>(&self, work: W, mut renew: R) -> Result<T, SandboxStoreError>
    where
        W: Future<Output = Result<T, SandboxStoreError>>,
        R: FnMut() -> F,
        F: Future<Output = Result<Option<WorkLeaseGrant>, SandboxStoreError>>,
    {
        let mut chain = Self::current()?;
        chain.push(self.clone());
        let _closed = CloseOnDrop(Arc::clone(&self.validity));
        CURRENT.scope(chain, async {
            let mut next = self.heartbeat(&mut renew).await?;
            tokio::pin!(work);
            loop {
                let deadline = self.deadline()?;
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(deadline) => return Err(SandboxStoreError::Conflict),
                    _ = tokio::time::sleep_until(next) => {
                        // Publication and renewal can contend on the same row.
                        // Let work holding that row proceed under the current grant.
                        tokio::select! {
                            biased;
                            _ = tokio::time::sleep_until(deadline) => return Err(SandboxStoreError::Conflict),
                            renewal = self.heartbeat(&mut renew) => next = renewal?,
                            result = &mut work => return result,
                        }
                    }
                    result = &mut work => return result,
                }
            }
        }).await
    }
}

struct CloseOnDrop(Arc<Validity>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;

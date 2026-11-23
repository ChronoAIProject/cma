use std::{future::Future, time::Duration};

use layer2_core::{
    sandbox::{
        store::{SandboxMutationGuard, WORK_RENEW_SECONDS},
        Sandbox,
    },
    seconds_from_rfc3339,
};
use layer3_adapter::{AdapterError, Result};
use tokio::time::{sleep_until, timeout_at, Instant};

use super::{refused, SandboxWorker};

const READ_INTERVAL: Duration = Duration::from_secs(WORK_RENEW_SECONDS);

/// The caller renews its Sandbox claim. The owner may follow a renewal only
/// while its last observed authority is live and the same claim still matches.
pub(super) struct Claim<'a> {
    worker: &'a SandboxWorker,
    guard: &'a SandboxMutationGuard,
    expires: Instant,
}

impl<'a> Claim<'a> {
    pub(super) fn new(
        worker: &'a SandboxWorker,
        guard: &'a SandboxMutationGuard,
        sandbox: &Sandbox,
    ) -> Result<Self> {
        let expires = expiry(sandbox).ok_or_else(refused)?;
        Ok(Self::from_observation(worker, guard, expires))
    }

    pub(super) fn from_observation(
        worker: &'a SandboxWorker,
        guard: &'a SandboxMutationGuard,
        expires: Instant,
    ) -> Self {
        Self {
            worker,
            guard,
            expires,
        }
    }

    pub(super) async fn follow<T>(
        &mut self,
        operation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(operation);
        loop {
            let expires = self.expires;
            tokio::select! {
                biased;
                _ = sleep_until(expires) => return Err(refused()),
                _ = sleep_until(Instant::now() + READ_INTERVAL) => {
                    let parent = self.read().await?;
                    self.observe(&parent)?;
                }
                result = &mut operation => {
                    let result = result?;
                    let parent = self.read().await?;
                    self.observe(&parent)?;
                    return Ok(result);
                }
            }
        }
    }

    pub(super) async fn read(&self) -> Result<Sandbox> {
        self.read_while(
            self.worker
                .dependencies
                .store
                .get_sandbox(&self.guard.sandbox_id),
        )
        .await?
        .ok_or_else(refused)
    }

    pub(super) async fn read_while<T, E>(
        &self,
        request: impl Future<Output = std::result::Result<T, E>>,
    ) -> Result<T> {
        if Instant::now() >= self.expires {
            return Err(refused());
        }
        let result = timeout_at(self.expires.min(Instant::now() + READ_INTERVAL), request)
            .await
            .map_err(|_| refused())?
            .map_err(|_| {
                AdapterError::Unavailable("lifecycle reopen claim could not be read".into())
            })?;
        // Tokio may return a ready store result before observing its timer.
        if Instant::now() >= self.expires {
            return Err(refused());
        }
        Ok(result)
    }

    pub(super) fn observe(&mut self, parent: &Sandbox) -> Result<()> {
        if Instant::now() >= self.expires || !self.guard.matches(parent) {
            return Err(refused());
        }
        self.expires = expiry(parent).ok_or_else(refused)?;
        Ok(())
    }

    pub(super) fn expires(&self) -> Instant {
        self.expires
    }
}

fn expiry(parent: &Sandbox) -> Option<Instant> {
    let expires = seconds_from_rfc3339(&parent.work.lease.as_ref()?.expires_at)?;
    let remaining = (std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(expires))
        .duration_since(std::time::SystemTime::now())
        .ok()
        .filter(|remaining| !remaining.is_zero())?;
    Instant::now().checked_add(remaining)
}

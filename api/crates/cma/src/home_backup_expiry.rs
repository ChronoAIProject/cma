//! Independent expiry work; never consumes the shared background budget.
use layer2_core::sandbox::home_backup::{
    HomeBackup, HomeBackupDeletionObservation, HomeBackupDeletionStore, HomeBackupError,
};
use std::{sync::Arc, time::Duration};

#[cfg(test)]
mod orphan_tests;
pub(crate) mod orphans;
mod supervision;
#[cfg(test)]
mod tests;

const LEASE_SECONDS: u64 = 60;
const STEP_SECONDS: u64 = 20;
const STORE_SECONDS: u64 = 5;

/// Composition supplies the real deletion implementation only after its exact
/// retained-copy identity and independent absence semantics are available.
#[async_trait::async_trait]
pub(crate) trait DeletionProvider: Send + Sync {
    async fn delete_copy(&self, claimed: &HomeBackup) -> Result<(), HomeBackupError>;
    async fn observe_deletion(
        &self,
        claimed: &HomeBackup,
    ) -> Result<HomeBackupDeletionObservation, HomeBackupError>;
}

pub(crate) struct ExpiryWorker {
    enabled: bool,
    store: Arc<dyn HomeBackupDeletionStore>,
    provider: Option<Arc<dyn DeletionProvider>>,
    orphans: Option<tokio::sync::Mutex<orphans::OrphanScanner>>,
    holder: String,
}

impl ExpiryWorker {
    pub(crate) fn new(
        enabled: bool,
        store: Arc<dyn HomeBackupDeletionStore>,
        provider: Option<Arc<dyn DeletionProvider>>,
        orphans: Option<orphans::OrphanSources>,
        deployment: &str,
    ) -> Result<Arc<Self>, String> {
        if enabled && provider.is_none() {
            return Err("home backup expiry requires the verified deletion provider".into());
        }
        Ok(Arc::new(Self {
            enabled,
            store,
            provider,
            orphans: orphans
                .map(orphans::OrphanScanner::new)
                .map(tokio::sync::Mutex::new),
            holder: format!("{deployment}:{}", uuid::Uuid::new_v4()),
        }))
    }

    async fn reconcile_orphan_page(&self) -> Result<(), HomeBackupError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(scanner) = &self.orphans else {
            return Ok(());
        };
        scanner
            .lock()
            .await
            .reconcile_page(&layer2_core::now_rfc3339())
            .await?;
        Ok(())
    }

    async fn reconcile_one(&self) -> Result<bool, HomeBackupError> {
        self.reconcile_one_with_clock(&std::time::SystemTime::now)
            .await
    }

    async fn reconcile_one_with_clock(
        &self,
        clock: &(dyn Fn() -> std::time::SystemTime + Sync),
    ) -> Result<bool, HomeBackupError> {
        let Some(provider) = self.provider.as_ref() else {
            return if self.enabled {
                Err(HomeBackupError::Unavailable)
            } else {
                Ok(false)
            };
        };
        let at = clock();
        let now = layer2_core::rfc3339_at(at);
        let until = layer2_core::rfc3339_at(at + Duration::from_secs(LEASE_SECONDS));
        Self::remaining_until(&until, Duration::from_secs(STORE_SECONDS), clock())?;
        let claimed = tokio::time::timeout(Duration::from_secs(STORE_SECONDS), async {
            if self.enabled {
                self.store.claim_expiry(&now, &self.holder, &until).await
            } else {
                self.store.claim_expiring(&now, &self.holder, &until).await
            }
        })
        .await
        .map_err(|_| HomeBackupError::Unavailable)??;
        let Some(claimed) = claimed else {
            return Ok(false);
        };
        let provider_budget = Duration::from_secs(STEP_SECONDS + STORE_SECONDS);
        Self::remaining(&claimed, provider_budget, clock())?;
        tokio::time::timeout(
            Duration::from_secs(STEP_SECONDS),
            provider.delete_copy(&claimed),
        )
        .await
        .map_err(|_| HomeBackupError::Unavailable)??;
        Self::remaining(&claimed, provider_budget, clock())?;
        let observed = tokio::time::timeout(
            Duration::from_secs(STEP_SECONDS),
            provider.observe_deletion(&claimed),
        )
        .await
        .map_err(|_| HomeBackupError::Unavailable)??;
        // An accepted delete request or a named-object 404 is not copy absence.
        if !observed.absent {
            return Err(HomeBackupError::Unavailable);
        }
        let at = clock();
        Self::remaining(&claimed, Duration::from_secs(STORE_SECONDS), at)?;
        let now = layer2_core::rfc3339_at(at);
        tokio::time::timeout(
            Duration::from_secs(STORE_SECONDS),
            self.store.save_deletion(&claimed, &observed, &now),
        )
        .await
        .map_err(|_| HomeBackupError::Unavailable)??;
        Ok(true)
    }

    fn remaining(
        claimed: &HomeBackup,
        budget: Duration,
        now: std::time::SystemTime,
    ) -> Result<(), HomeBackupError> {
        let expires = claimed
            .lease
            .as_ref()
            .map(|lease| lease.expires_at.as_str())
            .ok_or(HomeBackupError::Conflict)?;
        Self::remaining_until(expires, budget, now)
    }

    fn remaining_until(
        expires: &str,
        budget: Duration,
        now: std::time::SystemTime,
    ) -> Result<(), HomeBackupError> {
        let expires = layer2_core::seconds_from_rfc3339(expires)
            .and_then(|seconds| std::time::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .ok_or(HomeBackupError::Conflict)?;
        if !expires
            .duration_since(now)
            .is_ok_and(|remaining| remaining > budget)
        {
            return Err(HomeBackupError::Conflict);
        }
        Ok(())
    }
}

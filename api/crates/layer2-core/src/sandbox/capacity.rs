//! Owner quota observations, independent of runtime or Agent inventory.
use std::collections::BTreeMap;

use super::{service, SandboxService, SandboxServiceError, SandboxStatus};
use crate::{Caller, Capability};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxQuotaUsage {
    pub active: u64,
    pub retiring_charged: u64,
    pub retiring_released: u64,
}

#[derive(Debug, Clone)]
pub struct SandboxCapacitySnapshot {
    pub counts: Vec<(SandboxStatus, u64)>,
    pub retiring_released: u64,
    pub public_owner_limit: Option<u16>,
    pub observed_at: String,
}

#[derive(Debug, Clone)]
pub struct OwnerSandboxCapacity {
    pub limit: u16,
    pub live: u64,
    pub available: u64,
    pub counts_by_status: BTreeMap<String, u64>,
    pub retiring_released: u64,
    pub observed_at: String,
}

impl OwnerSandboxCapacity {
    pub fn quota_usage(&self) -> SandboxQuotaUsage {
        SandboxQuotaUsage {
            active: self.counts_by_status.get("active").copied().unwrap_or(0),
            retiring_charged: self.counts_by_status.get("retiring").copied().unwrap_or(0),
            retiring_released: self.retiring_released,
        }
    }
}

impl SandboxService {
    pub async fn capacity(
        &self,
        caller: &Caller,
    ) -> Result<OwnerSandboxCapacity, SandboxServiceError> {
        service::require(caller, Capability::UseOwnSandboxes)?;
        let policy = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(service::policy_error)?;
        let snapshot = self
            .store
            .owner_capacity(&caller.subject)
            .await
            .map_err(service::store_error)?;
        let mut counts_by_status = BTreeMap::new();
        let mut live = 0_u64;
        for (status, count) in snapshot.counts {
            if status == SandboxStatus::Retired
                || count == 0
                || counts_by_status
                    .insert(status.name().to_owned(), count)
                    .is_some()
            {
                return Err(SandboxServiceError::Unavailable(
                    "inconsistent Sandbox capacity".into(),
                ));
            }
            live = live.checked_add(count).ok_or_else(|| {
                SandboxServiceError::Unavailable("Sandbox capacity overflow".into())
            })?;
        }
        let owner_limit = policy.max_sandboxes_per_owner;
        let limit = owner_limit.min(snapshot.public_owner_limit.unwrap_or(owner_limit));
        Ok(OwnerSandboxCapacity {
            limit,
            live,
            available: u64::from(limit).saturating_sub(live),
            counts_by_status,
            retiring_released: snapshot.retiring_released,
            observed_at: snapshot.observed_at,
        })
    }
}

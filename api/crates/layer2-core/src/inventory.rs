//! Provider-neutral runtime inventory snapshots.

use serde::{Deserialize, Serialize};

use crate::StoreError;

/// Provider lifecycle state visible to reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Starting,
    Ready,
    Unavailable,
}

/// One provider runtime without assumptions about its owning aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEntry {
    pub runtime_id: String,
    pub generation: u64,
    pub owner_reference: Option<String>,
    pub status: RuntimeStatus,
}

impl RuntimeEntry {
    pub fn validate(&self) -> Result<(), crate::CoreError> {
        if self.runtime_id.trim().is_empty() || self.generation == 0 {
            return Err(crate::CoreError::InvalidRequest(
                "runtime inventory entries require an id and positive generation".into(),
            ));
        }
        Ok(())
    }
}

/// One complete provider inventory observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInventory {
    pub observed_at: String,
    pub runtimes: Vec<RuntimeEntry>,
}

impl ProviderInventory {
    pub fn validate(&self) -> Result<(), crate::CoreError> {
        if self.observed_at.trim().is_empty() {
            return Err(crate::CoreError::InvalidRequest(
                "runtime inventory requires an observation time".into(),
            ));
        }
        for runtime in &self.runtimes {
            runtime.validate()?;
        }
        let mut identities = std::collections::HashSet::new();
        if self
            .runtimes
            .iter()
            .any(|runtime| !identities.insert((&runtime.runtime_id, runtime.generation)))
        {
            return Err(crate::CoreError::InvalidState(
                "runtime inventory repeated an identity and generation".into(),
            ));
        }
        Ok(())
    }
}

/// Port through which reconciliation obtains a complete provider snapshot.
#[async_trait::async_trait]
pub trait RuntimeInventory: Send + Sync {
    async fn snapshot(&self) -> Result<ProviderInventory, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> RuntimeEntry {
        RuntimeEntry {
            runtime_id: "runtime-a".into(),
            generation: 1,
            owner_reference: None,
            status: RuntimeStatus::Ready,
        }
    }

    #[test]
    fn rejects_incomplete_and_duplicate_inventory_entries() {
        let mut inventory = ProviderInventory {
            observed_at: "2026-09-05T00:00:00Z".into(),
            runtimes: vec![entry()],
        };
        assert!(inventory.validate().is_ok());
        inventory.runtimes.push(entry());
        assert!(inventory.validate().is_err());
        inventory.runtimes[0].runtime_id.clear();
        assert!(inventory.validate().is_err());
    }
}

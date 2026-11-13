//! Authorization boundary for the retained activity log.

use std::sync::Arc;

use crate::{ActivityQuery, ActivityRecord, Caller, Capability, CoreError, Page, StoreError};

#[async_trait::async_trait]
pub trait ActivityStore: Send + Sync {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError>;
    /// Adapters may batch this operation; retries must preserve record IDs.
    async fn append_many(&self, records: &[ActivityRecord]) -> Result<(), StoreError> {
        for record in records {
            self.append(record).await?;
        }
        Ok(())
    }
    async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError>;
}

pub struct ActivityService {
    store: Arc<dyn ActivityStore>,
}

impl ActivityService {
    pub fn new(store: Arc<dyn ActivityStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    pub async fn record(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.store.append(record).await
    }

    pub async fn list(
        &self,
        caller: &Caller,
        query: &ActivityQuery,
    ) -> Result<Page<ActivityRecord>, CoreError> {
        caller.require(Capability::ReadActivityLog)?;
        query.validate()?;
        self.store
            .list(query)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}

#[cfg(test)]
#[path = "activity_service_tests.rs"]
mod tests;

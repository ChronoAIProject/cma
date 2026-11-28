//! Authorization boundary for the retained activity log.

use std::sync::Arc;

use crate::{
    ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord, ActivitySummary, Caller,
    Capability, CoreError, Page, StoreError,
};

#[async_trait::async_trait]
pub trait ActivityStore: Send + Sync {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError>;
    async fn attempt(&self, _id: &ActivityId) -> Result<Option<ActivityRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "activity lookup unavailable".into(),
        ))
    }
    async fn identify_attempt(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Err(StoreError::Unavailable(
            "activity identity updates are unavailable".into(),
        ))
    }
    async fn finish_attempt(
        &self,
        _id: &ActivityId,
        _outcome: &ActivityOutcome,
        _status: u16,
    ) -> Result<(), StoreError> {
        Err(StoreError::Unavailable(
            "activity outcome updates are unavailable".into(),
        ))
    }
    /// Adapters may batch this operation; retries must preserve record IDs.
    async fn append_many(&self, records: &[ActivityRecord]) -> Result<(), StoreError> {
        for record in records {
            self.append(record).await?;
        }
        Ok(())
    }
    async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError>;
    /// Compatibility adapters may project after loading; persisted adapters should narrow the read.
    async fn list_summaries(
        &self,
        query: &ActivityQuery,
    ) -> Result<Page<ActivitySummary>, StoreError> {
        let page = self.list(query).await?;
        Ok(Page::new(
            page.items.into_iter().map(ActivitySummary::from).collect(),
            page.next_cursor,
        ))
    }
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

    pub async fn attempt(&self, id: &ActivityId) -> Result<Option<ActivityRecord>, StoreError> {
        self.store.attempt(id).await
    }

    pub async fn identify_attempt(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.store.identify_attempt(record).await
    }

    pub async fn finish_attempt(
        &self,
        id: &ActivityId,
        outcome: &ActivityOutcome,
        status: u16,
    ) -> Result<(), StoreError> {
        self.store.finish_attempt(id, outcome, status).await
    }

    pub async fn list_summaries(
        &self,
        caller: &Caller,
        query: &ActivityQuery,
    ) -> Result<Page<ActivitySummary>, CoreError> {
        caller.require(Capability::ReadActivityLog)?;
        query.validate()?;
        self.store
            .list_summaries(query)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
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

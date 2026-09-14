//! Durable append-only event-log boundary.

use std::sync::Arc;

use crate::{CoreError, Event, EventKind, Sequences, StoreError};

#[async_trait::async_trait]
pub trait EventStore: Send + Sync {
    async fn tail(&self, resource_id: &str) -> Result<u64, StoreError>;
    async fn append(&self, event: &Event) -> Result<(), StoreError>;
    async fn list(
        &self,
        resource_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError>;
}

/// Coordinates process-local sequence reservations with one durable event store.
pub struct EventLog {
    store: Arc<dyn EventStore>,
    sequences: Arc<Sequences>,
}

impl EventLog {
    pub fn new(store: Arc<dyn EventStore>) -> Self {
        Self {
            store,
            sequences: Arc::new(Sequences::default()),
        }
    }

    pub async fn append(&self, resource_id: &str, kind: EventKind) -> Result<Event, CoreError> {
        if resource_id.trim().is_empty() {
            return Err(CoreError::InvalidRequest(
                "an event log requires a resource id".into(),
            ));
        }
        let tail = self.store.tail(resource_id).await.map_err(store_failed)?;
        let sequence = self.sequences.next(resource_id, tail);
        let event = Event::new(resource_id, sequence, kind)?;
        self.store.append(&event).await.map_err(store_failed)?;
        self.sequences.observe(resource_id, sequence);
        Ok(event)
    }

    pub async fn list(
        &self,
        resource_id: &str,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>, CoreError> {
        if resource_id.trim().is_empty() || !(1..=crate::MAX_PAGE_SIZE).contains(&limit) {
            return Err(CoreError::InvalidRequest(
                "event reads require a resource id and bounded limit".into(),
            ));
        }
        self.store
            .list(resource_id, after, limit)
            .await
            .map_err(store_failed)
    }
}

fn store_failed(error: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<Vec<Event>>);

    #[async_trait::async_trait]
    impl EventStore for MemoryStore {
        async fn tail(&self, resource_id: &str) -> Result<u64, StoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.resource_id == resource_id)
                .map(|event| event.sequence)
                .max()
                .unwrap_or_default())
        }

        async fn append(&self, event: &Event) -> Result<(), StoreError> {
            self.0.lock().unwrap().push(event.clone());
            Ok(())
        }

        async fn list(
            &self,
            resource_id: &str,
            after: u64,
            limit: u32,
        ) -> Result<Vec<Event>, StoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.resource_id == resource_id && event.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }
    }

    #[tokio::test]
    async fn append_and_read_share_one_monotonic_boundary() {
        let log = EventLog::new(Arc::new(MemoryStore::default()));
        assert_eq!(
            log.append("resource-a", EventKind::Created)
                .await
                .unwrap()
                .sequence,
            1
        );
        assert_eq!(
            log.append("resource-a", EventKind::Updated)
                .await
                .unwrap()
                .sequence,
            2
        );
        assert_eq!(log.list("resource-a", 1, 10).await.unwrap().len(), 1);
        assert!(log.list("", 0, 10).await.is_err());
    }
}

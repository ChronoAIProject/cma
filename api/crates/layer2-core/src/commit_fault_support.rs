//! A session store that faults only the semantic commit a test names.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::coordination::{RuntimeAuthority, SessionInteractionLeaseRequest, SessionLease};
use crate::event::{Event, StoredEvent};
use crate::ids::{SessionId, SubjectId};
use crate::store::{Page, SessionListQuery, SessionRecord, SessionStore, StoreError};
use crate::test_support::FakeStore;
use crate::Reserved;

/// Wraps the canonical fake and faults only commits selected by `matches`.
pub(crate) struct CommitFaultStore {
    pub(crate) inner: Arc<FakeStore>,
    matches: fn(&SessionRecord, &[Event]) -> bool,
    pause_next: AtomicBool,
    pause_after_next: AtomicBool,
    coordination_enabled: AtomicBool,
    failures: AtomicUsize,
    matched_calls: AtomicUsize,
    matched_write_ids: StdMutex<Vec<String>>,
    entered: Notify,
    release: Notify,
}

impl CommitFaultStore {
    pub(crate) fn new(
        inner: Arc<FakeStore>,
        matches: fn(&SessionRecord, &[Event]) -> bool,
    ) -> Self {
        Self {
            inner,
            matches,
            pause_next: AtomicBool::new(false),
            pause_after_next: AtomicBool::new(false),
            coordination_enabled: AtomicBool::new(false),
            failures: AtomicUsize::new(0),
            matched_calls: AtomicUsize::new(0),
            matched_write_ids: StdMutex::new(Vec::new()),
            entered: Notify::new(),
            release: Notify::new(),
        }
    }

    pub(crate) fn pause_next(&self) {
        assert!(!self.pause_next.swap(true, Ordering::SeqCst));
    }

    pub(crate) fn pause_after_next(&self) {
        assert!(!self.pause_after_next.swap(true, Ordering::SeqCst));
    }

    pub(crate) fn fail_next(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    /// Preserve compatibility setup, then expose the wrapped store's interaction fence.
    pub(crate) fn enable_interaction_coordination(&self) {
        self.coordination_enabled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn matched_calls(&self) -> usize {
        self.matched_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn matched_write_ids(&self) -> Vec<String> {
        self.matched_write_ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) async fn wait_until_held(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .expect("the selected commit reached the store");
    }

    pub(crate) fn release(&self) {
        self.release.notify_one();
    }

    async fn before_commit(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<(), StoreError> {
        if !(self.matches)(record, events) {
            return Ok(());
        }
        self.matched_calls.fetch_add(1, Ordering::SeqCst);
        self.matched_write_ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(write_id.to_owned());
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(StoreError::Unavailable(
                "selected test commit is unavailable".into(),
            ));
        }
        Ok(())
    }

    async fn after_commit(&self, record: &SessionRecord, events: &[Event]) {
        if (self.matches)(record, events) && self.pause_after_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl SessionStore for CommitFaultStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.inner.ping().await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.inner.get(id).await
    }

    async fn runtime_authority(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeAuthority>, StoreError> {
        if !self.coordination_enabled.load(Ordering::SeqCst) {
            return Err(StoreError::CoordinationDisabled);
        }
        self.inner.runtime_authority(id).await
    }

    async fn acquire_interaction_lease(
        &self,
        request: &SessionInteractionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        if !self.coordination_enabled.load(Ordering::SeqCst) {
            return Err(StoreError::CoordinationDisabled);
        }
        self.inner.acquire_interaction_lease(request).await
    }

    async fn renew_interaction_lease(
        &self,
        lease: &SessionLease,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        if !self.coordination_enabled.load(Ordering::SeqCst) {
            return Err(StoreError::CoordinationDisabled);
        }
        self.inner.renew_interaction_lease(lease, ttl).await
    }

    async fn release_session_lease(&self, lease: &SessionLease) -> Result<(), StoreError> {
        if !self.coordination_enabled.load(Ordering::SeqCst) {
            return Err(StoreError::CoordinationDisabled);
        }
        self.inner.release_session_lease(lease).await
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_for(owner, query).await
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        self.inner.reserved_capacity().await
    }

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.inner.list_all(query).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.inner.delete(id).await
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.inner.append_events(events).await
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.before_commit(write_id, record, events).await?;
        let revision = self
            .inner
            .commit_session_idempotent(write_id, record, events)
            .await?;
        self.after_commit(record, events).await;
        Ok(revision)
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.before_commit(write_id, record, events).await?;
        let revision = self
            .inner
            .create_session_idempotent(write_id, record, events)
            .await?;
        self.after_commit(record, events).await;
        Ok(revision)
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.inner.events_since(id, after).await
    }
}

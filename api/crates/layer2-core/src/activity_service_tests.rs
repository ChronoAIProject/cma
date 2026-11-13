use super::*;
use crate::{ActivityAction, ActivityOutcome};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

#[derive(Default)]
struct Store {
    records: Mutex<Vec<ActivityRecord>>,
    fail_at: AtomicUsize,
}
#[async_trait::async_trait]
impl ActivityStore for Store {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        let mut records = self.records.lock().unwrap();
        if records.len() == self.fail_at.load(Ordering::Relaxed) {
            return Err(StoreError::Unavailable("injected".into()));
        }
        records.push(record.clone());
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn activity_batch_default_preserves_order_and_propagates_first_failure() {
    let records: Vec<_> = (0..3)
        .map(|_| ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded))
        .collect();
    let store = Store::default();
    assert!(store.append_many(&[]).await.is_ok());
    store.fail_at.store(1, Ordering::Relaxed);
    assert!(store.append_many(&records).await.is_err());
    assert_eq!(*store.records.lock().unwrap(), records[..1]);
    store.records.lock().unwrap().clear();
    store.fail_at.store(usize::MAX, Ordering::Relaxed);
    store.append_many(&records).await.unwrap();
    assert_eq!(*store.records.lock().unwrap(), records);
}

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

#[tokio::test]
async fn summary_fallback_preserves_fields_and_authorizes_before_store_access() {
    use crate::{ActivitySummary, Authorization, DetailKey, SubjectId};
    struct ReadStore(ActivityRecord);
    #[async_trait::async_trait]
    impl ActivityStore for ReadStore {
        async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
            unreachable!()
        }
        async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
            if query.actor.is_some() {
                return Err(StoreError::Unavailable("injected".into()));
            }
            Ok(Page::new(vec![self.0.clone()], Some("123:act_next".into())))
        }
    }
    let mut row = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded);
    row.detail
        .record(DetailKey::ResourceName, "retained detail");
    let expected = ActivitySummary::from(row.clone());
    let service = ActivityService::new(Arc::new(ReadStore(row)));
    let mut caller = Caller {
        subject: SubjectId::new("operator"),
        authorization: Authorization::new([]),
    };
    let query = ActivityQuery::default();
    assert!(matches!(
        service.list_summaries(&caller, &query).await,
        Err(CoreError::Forbidden)
    ));
    caller.authorization = Authorization::new([Capability::ReadActivityLog]);
    let result = service.list_summaries(&caller, &query).await.unwrap();
    assert_eq!(result.items, vec![expected]);
    assert_eq!(result.next_cursor.as_deref(), Some("123:act_next"));
    assert_eq!(
        service.list(&caller, &query).await.unwrap().items[0]
            .detail
            .get(DetailKey::ResourceName),
        Some("retained detail")
    );
    assert!(service
        .list_summaries(
            &caller,
            &ActivityQuery {
                actor: Some(SubjectId::new("fail")),
                ..query.clone()
            }
        )
        .await
        .is_err());
    for cursor in ["", "bad", "1:", "x:id"] {
        assert!(matches!(
            service
                .list_summaries(
                    &caller,
                    &ActivityQuery {
                        cursor: Some(cursor.into()),
                        ..query.clone()
                    }
                )
                .await,
            Err(CoreError::InvalidRequest(_))
        ));
    }
    for limit in [0, 201] {
        assert!(service
            .list_summaries(
                &caller,
                &ActivityQuery {
                    limit,
                    ..query.clone()
                }
            )
            .await
            .is_err());
    }
}

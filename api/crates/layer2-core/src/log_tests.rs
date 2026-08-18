//! What happens to a batch the store refuses.
//!
//! The defect these exist for was silence. A batch whose sequence the session
//! already held was rejected, the rejection was logged, and the events were
//! dropped — a turn that had already run, unrecoverable by any route, because
//! the durable log is what a client is told to resume from.
//!
//! Driven through the store rather than around it: what is under test is the
//! decision made when a write is refused, and the refusal has to be a real one
//! for the decision to mean anything.

use std::sync::Arc;

use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::store::{SessionStore, StoreError};
use crate::test_support::{service, FakeAdapter, FakeStore};

fn session() -> SessionId {
    SessionId("sesn_1".into())
}

fn event(seq: u64) -> Event {
    Event {
        session_id: session(),
        seq,
        kind: EventKind::Progress {
            stage: crate::session::ProvisioningStage::Ready,
        },
    }
}

/// A message, so an event can be told from another by its content.
fn said(seq: u64, content: &str) -> Event {
    Event {
        session_id: session(),
        seq,
        kind: EventKind::MessageCommitted {
            content: content.to_string(),
        },
    }
}

async fn logged(store: &Arc<FakeStore>) -> Vec<(u64, String)> {
    store
        .events_since(&session(), 0)
        .await
        .expect("readable")
        .into_iter()
        .map(|e| (e.seq, format!("{:?}", e.kind)))
        .collect()
}

#[tokio::test]
async fn a_refused_batch_is_re_numbered_rather_than_dropped() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    // Something else already holds this number — the production shape exactly:
    // provisioning's `ready` written while a turn was being folded.
    store
        .append_events(&[event(9)])
        .await
        .expect("the first writer wins");

    svc.append_durably(&session(), vec![said(9, "hi back")])
        .await
        .expect("the turn must still reach the log");

    let held = logged(&store).await;
    assert_eq!(held.len(), 2, "both events are there: {held:?}");
    assert!(
        held.iter().any(|(_, kind)| kind.contains("hi back")),
        "the turn's own event must be one of them: {held:?}"
    );
    let seqs: Vec<u64> = held.iter().map(|(s, _)| *s).collect();
    assert_eq!(seqs, vec![9, 10], "and it is numbered above what was there");
}

#[tokio::test]
async fn what_the_store_already_accepted_is_not_written_a_second_time() {
    // The trap in retrying. Inserts are ordered, so a batch that fails partway
    // has already had its opening events accepted. Re-numbering the whole
    // batch would put those in the log twice under two numbers, and nothing
    // would reject the duplicates — a client replaying would see the same
    // thing said twice.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    store.append_events(&[event(3)]).await.expect("written");

    svc.append_durably(
        &session(),
        vec![said(1, "first"), said(2, "second"), said(3, "third")],
    )
    .await
    .expect("the remainder must land");

    let held = logged(&store).await;
    let firsts = held.iter().filter(|(_, k)| k.contains("first")).count();
    assert_eq!(firsts, 1, "the accepted event is written once: {held:?}");
    assert_eq!(held.len(), 4, "one collision, nothing lost: {held:?}");
    let seqs: Vec<u64> = held.iter().map(|(s, _)| *s).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn re_numbering_clears_everything_the_log_has_reached() {
    // A collision is evidence that something else is writing, so our own
    // position is behind. Numbering from it again would collide again.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    for seq in 5..=9 {
        store.append_events(&[event(seq)]).await.expect("written");
    }

    svc.append_durably(&session(), vec![said(5, "carried across")])
        .await
        .expect("must land");

    let carried = logged(&store)
        .await
        .into_iter()
        .find(|(_, k)| k.contains("carried across"))
        .expect("it is in the log");
    assert_eq!(
        carried.0, 10,
        "numbered above the highest the log holds, not above the one that clashed"
    );
}

#[tokio::test]
async fn a_store_that_is_down_is_reported_rather_than_re_numbered() {
    // Renumbering a batch that was never refused would move events for a
    // reason that has nothing to do with sequences. An outage is propagated so
    // the caller decides.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    store.fail_everything().await;

    let outcome = svc.append_durably(&session(), vec![said(1, "hi")]).await;

    assert!(
        matches!(outcome, Err(StoreError::Unavailable(_))),
        "got {outcome:?}"
    );
}

#[tokio::test]
async fn an_empty_batch_is_not_a_write() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    store.fail_everything().await;

    svc.append_durably(&session(), Vec::new())
        .await
        .expect("nothing to write cannot fail");
}

/// A store that claims to have accepted more than it was given.
///
/// A store is a port, and an implementation is a trust boundary like any
/// other. One that miscounts must not cost the batch or take the process with
/// it — `drain` past the end panics, and a panicking control plane is a worse
/// outcome than any numbering mistake.
struct Miscounting(Arc<FakeStore>);

#[async_trait::async_trait]
impl SessionStore for Miscounting {
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        Err(StoreError::SequenceTaken {
            written: events.len() + 5,
            total: events.len(),
        })
    }

    async fn upsert(&self, record: &crate::store::SessionRecord) -> Result<(), StoreError> {
        self.0.upsert(record).await
    }
    async fn get(&self, id: &SessionId) -> Result<Option<crate::store::SessionRecord>, StoreError> {
        self.0.get(id).await
    }
    async fn list_for(
        &self,
        owner: &crate::ids::SubjectId,
    ) -> Result<Vec<crate::store::SessionRecord>, StoreError> {
        self.0.list_for(owner).await
    }
    async fn list_all(&self) -> Result<Vec<crate::store::SessionRecord>, StoreError> {
        self.0.list_all().await
    }
    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.0.delete(id).await
    }
    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<crate::event::StoredEvent>, StoreError> {
        self.0.events_since(id, after).await
    }
}

#[tokio::test]
async fn a_store_that_miscounts_costs_neither_the_batch_nor_the_process() {
    let svc = crate::test_support::service_over(
        Arc::new(Miscounting(Arc::new(FakeStore::default()))),
        Arc::new(FakeAdapter::default()),
    );

    let outcome = svc
        .append_durably(&session(), vec![said(1, "hi"), said(2, "there")])
        .await;

    assert!(
        matches!(outcome, Err(StoreError::SequenceTaken { .. })),
        "reported rather than silently succeeded: {outcome:?}"
    );
}

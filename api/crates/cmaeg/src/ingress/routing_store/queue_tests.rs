use super::{test_server::Server, *};
use crate::ingress::routing_plan::fixture;
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
struct Queue {
    unavailable: AtomicBool,
    calls: AtomicUsize,
    admitted: Mutex<Vec<AdmittedPlan>>,
}
impl AdmissionQueue for Queue {
    fn enqueue<'a>(&'a self, plan: &'a AdmittedPlan) -> BoxFuture<'a, Result<(), AdmissionError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(AdmissionError::Uncertain);
            }
            self.admitted.lock().unwrap().push(plan.clone());
            Ok(())
        })
    }
}

fn candidate(text: &str) -> (crate::ingress::launch::LaunchRequest, Candidate) {
    let request = fixture::request(fixture::targets, fixture::message(text));
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"verified-original-body",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    (request, candidate)
}

#[tokio::test]
async fn replica_acceptance_and_replay_require_queue_custody_but_reads_do_not_enqueue() {
    let server = Server::start().await;
    let queue = Arc::new(Queue::default());
    let store = server.store().with_admission_queue(queue.clone());
    let (request, candidate) = candidate("original");
    queue.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        store.admit(&candidate).await,
        Err(AdmissionError::Uncertain)
    );
    let retained = store.load(candidate.identity()).await.unwrap().unwrap();
    assert!(queue.admitted.lock().unwrap().is_empty());
    assert_eq!(queue.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.replay(&request, b"verified-original-body").await,
        Err(AdmissionError::Uncertain)
    );
    queue.unavailable.store(false, Ordering::SeqCst);
    assert_eq!(store.admit(&candidate).await.unwrap(), retained);
    assert_eq!(
        store
            .replay(&request, b"verified-original-body")
            .await
            .unwrap(),
        Some(retained.clone())
    );
    assert_eq!(*queue.admitted.lock().unwrap(), vec![retained.clone(); 2]);
    let count = queue.calls.load(Ordering::SeqCst);
    assert_eq!(
        store.load(candidate.identity()).await.unwrap(),
        Some(retained)
    );
    assert_eq!(queue.calls.load(Ordering::SeqCst), count);
}

#[tokio::test]
async fn changed_body_replay_cannot_enqueue_or_replace_the_original_admission() {
    let server = Server::start().await;
    let queue = Arc::new(Queue::default());
    let store = server.store().with_admission_queue(queue.clone());
    let (_, original) = candidate("original");
    let retained = store.admit(&original).await.unwrap();
    let (changed_request, changed) = candidate("changed");
    assert_eq!(store.admit(&changed).await, Err(AdmissionError::Conflict));
    assert_eq!(
        store
            .replay(&changed_request, b"verified-original-body")
            .await,
        Err(AdmissionError::Conflict)
    );
    assert_eq!(queue.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*queue.admitted.lock().unwrap(), vec![retained]);
}

#[tokio::test]
async fn restart_scan_repairs_committed_root_without_accepting_unqueued_work() {
    let server = Server::start().await;
    let (_, candidate) = candidate("original");
    // The prior process stopped after root commit, before the queue acknowledgement.
    let retained = server.store().admit(&candidate).await.unwrap();
    let queue = Arc::new(Queue::default());
    let restarted = server.store().with_admission_queue(queue.clone());
    queue.unavailable.store(true, Ordering::SeqCst);
    let page = restarted.recover_all_page(None, 32).await.unwrap();
    assert!(page.plans.is_empty());
    assert_eq!(page.issues.len(), 1);
    assert_eq!(page.issues[0].error, AdmissionError::Uncertain);
    assert_eq!(page.issues[0].record_id, retained.admission_id);
    queue.unavailable.store(false, Ordering::SeqCst);
    let page = restarted.recover_all_page(None, 32).await.unwrap();
    assert!(page.issues.is_empty());
    assert_eq!(page.plans, vec![retained.clone()]);
    assert_eq!(*queue.admitted.lock().unwrap(), vec![retained]);
}

#[tokio::test]
async fn failed_root_never_reaches_the_admission_queue() {
    let server = Server::start().await;
    let queue = Arc::new(Queue::default());
    let store = server.store().with_admission_queue(queue.clone());
    let (_, candidate) = candidate("original");
    server.fault(candidate.admission_id().into(), false, false);
    assert_eq!(
        store.admit(&candidate).await,
        Err(AdmissionError::Uncertain)
    );
    assert_eq!(queue.calls.load(Ordering::SeqCst), 0);
    assert!(store.load(candidate.identity()).await.unwrap().is_none());
    assert!(store.admit(&candidate).await.is_ok());
    assert_eq!(queue.calls.load(Ordering::SeqCst), 1);
}

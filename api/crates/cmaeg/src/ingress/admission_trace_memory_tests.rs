use super::*;
use crate::ingress::routing_plan::fixture;
use futures_util::FutureExt;
use std::{
    marker::PhantomData,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

#[derive(Default)]
struct Counts {
    polls: AtomicUsize,
    drops: AtomicUsize,
}

struct PendingWork<E> {
    bytes: [u8; 8 * 1024],
    counts: Arc<Counts>,
    error: PhantomData<E>,
}

impl<E> PendingWork<E> {
    fn new(counts: &Arc<Counts>) -> Self {
        Self {
            bytes: [0; 8 * 1024],
            counts: counts.clone(),
            error: PhantomData,
        }
    }
}

impl<E> Future for PendingWork<E> {
    type Output = Result<(), E>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        std::hint::black_box(&self.bytes);
        self.counts.polls.fetch_add(1, Ordering::Relaxed);
        Poll::Pending
    }
}

impl<E> Drop for PendingWork<E> {
    fn drop(&mut self) {
        self.counts.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn observations_bound_parent_state_without_eagerly_polling_work() {
    let counts = Arc::new(Counts::default());
    let request = fixture::request(fixture::targets, fixture::message("memory-bound"));
    let record = record(Stage::RootWrite, PendingWork::<RecordError>::new(&counts));
    let admission = admission(Stage::Enqueue, PendingWork::<AdmissionError>::new(&counts));
    let attempt = attempt(&request, PendingWork::<Problem>::new(&counts));

    for (name, bytes) in [
        ("record", std::mem::size_of_val(&record)),
        ("admission", std::mem::size_of_val(&admission)),
        ("attempt", std::mem::size_of_val(&attempt)),
    ] {
        assert!(bytes <= 1024, "{name} retains {bytes} bytes in its parent");
    }
    assert_eq!(counts.polls.load(Ordering::Relaxed), 0);
    drop((record, admission, attempt));
    assert_eq!(counts.drops.load(Ordering::Relaxed), 3);
}

#[test]
fn cancelling_nested_observations_drops_pending_work_once() {
    let counts = Arc::new(Counts::default());
    let request = fixture::request(fixture::targets, fixture::message("memory-bound"));
    let work = PendingWork::<RecordError>::new(&counts);
    let record = record(Stage::RootReadback, work);
    let admission = admission(Stage::RootReadback, async move {
        record.await.map_err(|_| AdmissionError::Unavailable)
    });
    let attempt = attempt(&request, async move {
        admission.await.map_err(|_| {
            Problem::new(
                cmaeg_core::problem::ProblemCode::Internal,
                500,
                "unavailable",
            )
        })
    });

    assert!(std::mem::size_of_val(&attempt) <= 1024);
    assert_eq!(counts.polls.load(Ordering::Relaxed), 0);
    assert!(attempt.now_or_never().is_none());
    assert_eq!(counts.polls.load(Ordering::Relaxed), 1);
    assert_eq!(counts.drops.load(Ordering::Relaxed), 1);
}

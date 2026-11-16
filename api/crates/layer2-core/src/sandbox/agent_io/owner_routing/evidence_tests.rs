use super::*;
use futures_util::FutureExt;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    task::{Context, Poll},
};

#[derive(Default)]
struct Observed {
    polls: AtomicUsize,
    drops: AtomicUsize,
    scopes: Mutex<Vec<Option<Vec<u8>>>>,
}

struct PendingWork {
    bytes: [u8; 8 * 1024],
    observed: Arc<Observed>,
}

impl PendingWork {
    fn new(observed: &Arc<Observed>) -> Self {
        Self {
            bytes: [0; 8 * 1024],
            observed: observed.clone(),
        }
    }

    fn record_scope(&self) {
        self.observed
            .scopes
            .lock()
            .unwrap()
            .push(OwnerCommandRoutingEvidence::current().map(|evidence| evidence.bytes().to_vec()));
    }
}

impl Future for PendingWork {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        std::hint::black_box(&self.bytes);
        self.observed.polls.fetch_add(1, Ordering::Relaxed);
        self.record_scope();
        Poll::Pending
    }
}

impl Drop for PendingWork {
    fn drop(&mut self) {
        self.observed.drops.fetch_add(1, Ordering::Relaxed);
        self.record_scope();
    }
}

fn evidence() -> OwnerCommandRoutingEvidence {
    OwnerCommandRoutingEvidence::new(Arc::from(&b"fixture-origin"[..]))
}

#[test]
fn request_scopes_bound_parent_state_without_polling_or_entering_early() {
    let observed = Arc::new(Observed::default());
    let evidence = evidence();
    let required = evidence.scope(PendingWork::new(&observed));
    let optional = OwnerCommandRoutingEvidence::scope_optional(
        Some(evidence.clone()),
        PendingWork::new(&observed),
    );
    let absent = OwnerCommandRoutingEvidence::scope_optional(None, PendingWork::new(&observed));
    for (name, bytes) in [
        ("required", std::mem::size_of_val(&required)),
        ("optional", std::mem::size_of_val(&optional)),
        ("absent", std::mem::size_of_val(&absent)),
    ] {
        assert!(bytes <= 1024, "{name} retains {bytes} bytes in its caller");
    }
    assert_eq!(observed.polls.load(Ordering::Relaxed), 0);
    assert!(OwnerCommandRoutingEvidence::current().is_none());
    drop((required, optional, absent));
    assert_eq!(observed.drops.load(Ordering::Relaxed), 3);
    assert_eq!(*observed.scopes.lock().unwrap(), vec![None; 3]);
}

#[test]
fn cancellation_drops_pending_work_once_under_its_original_request_scope() {
    let observed = Arc::new(Observed::default());
    let evidence = evidence();
    let future = OwnerCommandRoutingEvidence::scope_optional(
        Some(evidence.clone()),
        PendingWork::new(&observed),
    );
    assert!(future.now_or_never().is_none());
    assert_eq!(observed.polls.load(Ordering::Relaxed), 1);
    assert_eq!(observed.drops.load(Ordering::Relaxed), 1);
    assert_eq!(
        *observed.scopes.lock().unwrap(),
        vec![Some(evidence.bytes().to_vec()); 2]
    );
    assert!(OwnerCommandRoutingEvidence::current().is_none());
}

#[tokio::test]
async fn nested_request_scopes_restore_the_caller_and_preserve_errors() {
    let outer = evidence();
    let inner = OwnerCommandRoutingEvidence::new(Arc::from(&b"inner-origin"[..]));
    let result = outer
        .scope(async {
            let result = OwnerCommandRoutingEvidence::scope_optional(Some(inner.clone()), async {
                assert_eq!(
                    OwnerCommandRoutingEvidence::current().unwrap().bytes(),
                    inner.bytes()
                );
                Err::<(), _>("unchanged refusal")
            })
            .await;
            assert_eq!(
                OwnerCommandRoutingEvidence::current().unwrap().bytes(),
                outer.bytes()
            );
            result
        })
        .await;
    assert_eq!(result, Err("unchanged refusal"));
    assert!(OwnerCommandRoutingEvidence::current().is_none());
}

use super::run;
use std::{cell::RefCell, rc::Rc, time::Duration};
use tokio::sync::oneshot;

#[tokio::test]
async fn shutdown_joins_started_bindings_without_admitting_more() {
    let stopping = std::cell::Cell::new(false);
    let started = std::cell::Cell::new(0);
    let (senders, receivers): (Vec<_>, Vec<_>) = (0..4).map(|_| oneshot::channel()).unzip();
    let batch = run(
        receivers,
        2,
        || stopping.get(),
        |receive| async {
            started.set(started.get() + 1);
            Ok::<_, ()>(receive.await.unwrap())
        },
    );
    tokio::pin!(batch);
    assert!(futures_util::poll!(&mut batch).is_pending());
    assert_eq!(started.get(), 2);
    stopping.set(true);
    let mut senders = senders.into_iter();
    senders.next().unwrap().send(true).unwrap();
    assert!(futures_util::poll!(&mut batch).is_pending());
    assert_eq!(started.get(), 2);
    senders.next().unwrap().send(false).unwrap();
    let report = batch.await;
    assert_eq!((report.started, report.settled, report.deferred), (2, 2, 1));
    assert!(report.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn thirty_four_bindings_overlap_at_eight_and_keep_each_chain_in_order() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let counts = Rc::new(RefCell::new((0usize, 0usize)));
    let started = tokio::time::Instant::now();
    let report = run(
        0..34,
        8,
        || false,
        |id| {
            let events = events.clone();
            let counts = counts.clone();
            async move {
                {
                    let mut counts = counts.borrow_mut();
                    counts.0 += 1;
                    counts.1 = counts.1.max(counts.0);
                }
                for phase in ["proof", "status", "cas"] {
                    events.borrow_mut().push((id, phase));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                counts.borrow_mut().0 -= 1;
                Ok::<_, ()>(id != 7)
            }
        },
    )
    .await;
    assert_eq!(
        (report.started, report.settled, report.deferred),
        (34, 34, 1)
    );
    assert_eq!(report.peak, 8);
    assert_eq!(*counts.borrow(), (0, 8));
    assert!(report.error.is_none());
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    for id in 0..34 {
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter_map(|(found, phase)| (*found == id).then_some(*phase))
                .collect::<Vec<_>>(),
            ["proof", "status", "cas"]
        );
    }
}

struct Completion {
    done: bool,
    cancelled: Rc<RefCell<usize>>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        if !self.done {
            *self.cancelled.borrow_mut() += 1;
        }
    }
}

#[tokio::test]
async fn observed_error_closes_admission_and_drains_held_siblings_before_return() {
    let started = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Rc::new(RefCell::new(0));
    let (mut senders, receivers): (Vec<_>, Vec<_>) = (0..12)
        .map(|_| {
            let (send, receive) = oneshot::channel::<Result<bool, &'static str>>();
            (Some(send), receive)
        })
        .unzip();
    let batch = run(
        receivers.into_iter().enumerate(),
        8,
        || false,
        |(id, receive)| {
            let started = started.clone();
            let cancelled = cancelled.clone();
            async move {
                let mut completion = Completion {
                    done: false,
                    cancelled,
                };
                started.borrow_mut().push(id);
                let result = receive.await.unwrap();
                completion.done = true;
                result
            }
        },
    );
    tokio::pin!(batch);
    assert!(futures_util::poll!(&mut batch).is_pending());
    assert_eq!(started.borrow().len(), 8);
    senders[2]
        .take()
        .unwrap()
        .send(Err("first observed"))
        .unwrap();
    // Poll until the injected error is observed while every sibling is held.
    assert!(futures_util::poll!(&mut batch).is_pending());
    assert_eq!(started.borrow().len(), 8);
    senders[0].take().unwrap().send(Ok(false)).unwrap();
    assert!(futures_util::poll!(&mut batch).is_pending());
    assert_eq!(started.borrow().len(), 8);
    for sender in senders.iter_mut().take(8).filter_map(Option::take) {
        sender.send(Err("later error")).unwrap();
    }
    let report = batch.await;
    assert_eq!(report.error, Some("first observed"));
    assert_eq!((report.started, report.settled, report.deferred), (8, 8, 1));
    assert_eq!(*cancelled.borrow(), 0);
    assert_eq!(started.borrow().len(), 8);
}

#[tokio::test]
async fn serial_fallback_preserves_row_order_and_first_error_boundary() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let report = run(
        0..5,
        1,
        || false,
        |id| {
            let events = events.clone();
            async move {
                events.borrow_mut().push((id, "start"));
                tokio::task::yield_now().await;
                events.borrow_mut().push((id, "finish"));
                if id == 2 {
                    Err("stop")
                } else {
                    Ok(id != 0)
                }
            }
        },
    )
    .await;
    assert_eq!(report.error, Some("stop"));
    assert_eq!(report.peak, 1);
    assert_eq!((report.started, report.settled, report.deferred), (3, 3, 1));
    assert_eq!(
        *events.borrow(),
        [
            (0, "start"),
            (0, "finish"),
            (1, "start"),
            (1, "finish"),
            (2, "start"),
            (2, "finish")
        ]
    );
}

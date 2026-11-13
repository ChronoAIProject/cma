use super::*;
use std::{
    future::{poll_fn, Future},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::Context,
};

#[derive(Clone)]
struct ManualClock {
    base: Instant,
    nanos: Arc<AtomicU64>,
}
impl ManualClock {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            nanos: Arc::default(),
        }
    }
    fn advance(&self, duration: Duration) {
        self.nanos
            .fetch_add(duration.as_nanos().try_into().unwrap(), Ordering::Relaxed);
    }
}
impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}

#[tokio::test]
async fn immediately_ready_rows_remain_one_section_without_yielding() {
    let clock = ManualClock::new();
    let mut sections = ExecutionSections::new(clock.clone());
    for n in 0..100 {
        assert_eq!(sections.observe(std::future::ready(n), true).await, n);
        clock.advance(Duration::from_millis(1));
    }
    assert_eq!(
        sections.finish(),
        SectionStats {
            sections: 1,
            longest_section: Duration::from_millis(100),
            longest_poll: Duration::ZERO,
        }
    );
}

#[test]
fn actual_pending_splits_sections_and_excludes_executor_wait() {
    let clock = ManualClock::new();
    let mut sections = ExecutionSections::new(clock.clone());
    let mut polls = 0;
    {
        let future = poll_fn(|_| {
            polls += 1;
            clock.advance(Duration::from_millis(polls + 1));
            if polls == 1 {
                Poll::Pending
            } else {
                Poll::Ready(Ok::<_, ()>(7))
            }
        });
        let mut observed = Box::pin(sections.observe(future, true));
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        assert!(observed.as_mut().poll(&mut cx).is_pending());
        clock.advance(Duration::from_secs(3600));
        assert_eq!(observed.as_mut().poll(&mut cx), Poll::Ready(Ok(7)));
    }
    assert_eq!(
        sections.finish(),
        SectionStats {
            sections: 2,
            longest_section: Duration::from_millis(3),
            longest_poll: Duration::from_millis(3),
        }
    );
}

#[test]
fn cancellation_preserves_work_before_pending_without_counting_wait() {
    let clock = ManualClock::new();
    let mut sections = ExecutionSections::new(clock.clone());
    {
        let future = poll_fn(|_| {
            clock.advance(Duration::from_millis(2));
            Poll::<()>::Pending
        });
        let mut observed = Box::pin(sections.observe(future, true));
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        assert!(observed.as_mut().poll(&mut cx).is_pending());
    }
    clock.advance(Duration::from_secs(10));
    assert_eq!(
        sections.finish(),
        SectionStats {
            sections: 1,
            longest_section: Duration::from_millis(2),
            longest_poll: Duration::ZERO,
        }
    );
}

#[tokio::test]
async fn errors_keep_the_final_ready_poll_measurement() {
    let clock = ManualClock::new();
    let mut sections = ExecutionSections::new(clock.clone());
    let result = sections
        .observe(
            poll_fn(|_| {
                clock.advance(Duration::from_millis(4));
                Poll::Ready(Err::<(), _>("fixture failure"))
            }),
            true,
        )
        .await;
    assert_eq!(result, Err("fixture failure"));
    assert_eq!(sections.finish().longest_poll, Duration::from_millis(4));
}

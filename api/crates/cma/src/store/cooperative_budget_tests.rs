use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
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

async fn checkpoints(budget: &mut CooperativeBudget<ManualClock>) {
    for _ in 0..CLOCK_STRIDE {
        budget.checkpoint().await;
    }
}

#[tokio::test]
async fn under_budget_does_not_yield_and_exact_boundary_yields_once_then_resets() {
    let clock = ManualClock::new();
    let mut budget = CooperativeBudget::new(Policy::enabled(true), clock.clone());
    clock.advance(SECTION_BUDGET - Duration::from_nanos(1));
    assert!(!budget.should_yield(clock.now()));
    checkpoints(&mut budget).await;
    assert_eq!(budget.yields, 0);
    clock.advance(Duration::from_nanos(1));
    assert!(budget.should_yield(clock.now()));
    checkpoints(&mut budget).await;
    assert_eq!(budget.yields, 1);
    assert!(!budget.should_yield(clock.now()));
    checkpoints(&mut budget).await;
    assert_eq!(budget.yields, 1);
    assert_eq!(budget.finish().0.longest_section, SECTION_BUDGET);
}

#[tokio::test]
async fn disabled_policy_never_yields_even_when_far_over_budget() {
    let clock = ManualClock::new();
    let mut budget = CooperativeBudget::new(Policy::default(), clock.clone());
    clock.advance(Duration::from_secs(3600));
    assert!(!budget.should_yield(clock.now()));
    for _ in 0..100 {
        budget.checkpoint().await;
    }
    assert_eq!(budget.finish().1, 0);
}

#[tokio::test]
async fn enabled_loop_schedules_an_already_ready_task_before_it_finishes() {
    for enabled in [false, true] {
        let clock = ManualClock::new();
        let mut budget = CooperativeBudget::new(Policy::enabled(enabled), clock.clone());
        let ran = Arc::new(AtomicBool::new(false));
        let ready = ran.clone();
        let peer = tokio::spawn(async move {
            ready.store(true, Ordering::SeqCst);
        });
        for _ in 0..CLOCK_STRIDE {
            clock.advance(SECTION_BUDGET);
            budget.checkpoint().await;
        }
        assert_eq!(ran.load(Ordering::SeqCst), enabled);
        assert_eq!(budget.finish().1, u64::from(enabled));
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn pending_wait_resets_the_section_without_counting_it_as_explicit_yield() {
    let clock = ManualClock::new();
    let mut budget = CooperativeBudget::new(Policy::enabled(true), clock.clone());
    budget
        .observe(
            async {
                tokio::task::yield_now().await;
                clock.advance(Duration::from_micros(100));
            },
            true,
        )
        .await;
    assert!(!budget.should_yield(clock.now()));
    assert_eq!(budget.finish().1, 0);
}

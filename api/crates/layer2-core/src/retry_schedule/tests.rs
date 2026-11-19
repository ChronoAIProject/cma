use super::*;
use std::sync::Mutex;

struct Time(Mutex<Instant>);
impl Clock for Time {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}
struct Extra(Duration);
impl Jitter for Extra {
    fn extra(&self, _: Duration) -> Duration {
        self.0
    }
}

fn fixture(capacity: usize, extra: Duration) -> (RetrySchedule<u32>, Arc<Time>) {
    let time = Arc::new(Time(Mutex::new(Instant::now())));
    (
        RetrySchedule::new(
            capacity,
            RetryPolicy {
                initial: Duration::from_secs(3),
                maximum: Duration::from_secs(60),
                retention: Duration::from_secs(300),
            },
            time.clone(),
            Arc::new(Extra(extra)),
        ),
        time,
    )
}

#[test]
fn exponential_delays_and_jitter_are_bounded_even_for_hostile_policy_output() {
    for extra in [Duration::ZERO, Duration::MAX] {
        let (mut schedule, time) = fixture(2, extra);
        for (index, seconds) in [3, 6, 12, 24, 48, 60, 60, 60].into_iter().enumerate() {
            let (failures, delay) = schedule.failed(1);
            assert_eq!(failures, index as u32 + 1);
            let base = Duration::from_secs(seconds);
            let max = (base + (base / 10).min(Duration::from_secs(1))).min(Duration::from_secs(60));
            assert_eq!(delay, if extra.is_zero() { base } else { max });
            assert_eq!(schedule.remaining(&1), delay);
            *time.0.lock().unwrap() += delay - Duration::from_nanos(1);
            assert_eq!(schedule.remaining(&1), Duration::from_nanos(1));
            *time.0.lock().unwrap() += Duration::from_nanos(1);
            assert!(schedule.remaining(&1).is_zero());
        }
        schedule.entries.get_mut(&1).unwrap().failures = u32::MAX;
        assert_eq!(schedule.failed(1), (u32::MAX, Duration::from_secs(60)));
    }
}

#[test]
fn recovery_expiry_eviction_and_restart_only_remove_advisory_history() {
    let (mut schedule, time) = fixture(2, Duration::ZERO);
    schedule.failed(1);
    *time.0.lock().unwrap() += Duration::from_secs(1);
    schedule.failed(2);
    schedule.failed(3);
    assert_eq!(schedule.entries.len(), 2);
    assert!(!schedule.contains(&1));
    assert!(schedule.contains(&2) && schedule.contains(&3));
    schedule.recovered(&2);
    assert_eq!(schedule.failed(2).0, 1);
    *time.0.lock().unwrap() += Duration::from_secs(300);
    assert!(schedule.remaining(&2).is_zero());
    assert!(!schedule.contains(&2));
    assert!(schedule.entries.is_empty());
    assert!(fixture(2, Duration::ZERO).0.remaining(&2).is_zero());
    let (mut disabled, _) = fixture(0, Duration::ZERO);
    assert_eq!(disabled.failed(1), (0, Duration::ZERO));
    assert!(disabled.entries.is_empty());
}

#[test]
fn production_jitter_never_exceeds_its_supplied_budget() {
    for maximum in [
        Duration::ZERO,
        Duration::from_millis(300),
        Duration::from_secs(1),
    ] {
        for _ in 0..100 {
            assert!(RandomJitter.extra(maximum) <= maximum);
        }
    }
    assert!(MonotonicClock.now() <= Instant::now());
}

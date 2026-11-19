use super::{backoff, delay};
use std::time::Duration;
use tokio::time::Instant;

const SEEDS: [u64; 8] = [0, 1, 15, 16, 1 << 63, u64::MAX / 3, u64::MAX / 5, u64::MAX];

#[test]
fn first_attempt_has_no_delay_for_every_injected_seed() {
    for seed in SEEDS {
        assert_eq!(delay(0, seed), Duration::ZERO);
    }
}

#[test]
fn retries_add_at_most_fifteen_milliseconds_to_each_base() {
    for (attempt, base_ms) in [(1, 10), (2, 20), (3, 40), (4, 80)] {
        let bounds = Duration::from_millis(base_ms)..=Duration::from_millis(base_ms + 15);
        for seed in SEEDS {
            assert!(bounds.contains(&delay(attempt, seed)));
        }
    }
}

#[test]
fn later_attempts_keep_the_eighty_millisecond_base_cap() {
    let bounds = Duration::from_millis(80)..=Duration::from_millis(95);
    for attempt in [5, 6, 31, 32, 63, 64, 65, 255, u32::MAX as usize, usize::MAX] {
        for seed in SEEDS {
            assert!(bounds.contains(&delay(attempt, seed)));
        }
    }
}

#[test]
fn every_jitter_value_is_reachable_for_each_retry() {
    for attempt in (1..=128).chain([usize::MAX]) {
        let base = delay(attempt, 0);
        for jitter_ms in 0_u64..=15 {
            let seed = jitter_ms.rotate_right(attempt as u32);
            assert_eq!(
                delay(attempt, seed) - base,
                Duration::from_millis(jitter_ms)
            );
        }
    }
}

#[test]
fn five_attempts_have_a_maximum_total_backoff_of_210_milliseconds() {
    let total: Duration = (0..5)
        .map(|attempt| {
            (0_u64..=15)
                .map(|jitter_ms| delay(attempt, jitter_ms.rotate_right(attempt as u32)))
                .max()
                .unwrap()
        })
        .sum();
    assert_eq!(total, Duration::from_millis(210));
}

#[test]
fn shared_delay_matches_the_original_agent_publication_formula() {
    for attempt in (0..=128).chain([u32::MAX as usize, usize::MAX]) {
        for seed in SEEDS {
            let original = if attempt == 0 {
                Duration::ZERO
            } else {
                Duration::from_millis(
                    (10_u64 << (attempt - 1).min(3)) + (seed.rotate_left(attempt as u32) & 15),
                )
            };
            assert_eq!(delay(attempt, seed), original);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn first_attempt_does_not_advance_time() {
    let started = Instant::now();
    backoff(0).await;
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn retries_sleep_within_the_jittered_bounds() {
    for (attempt, base_ms) in [(1, 10), (2, 20), (3, 40), (4, 80), (5, 80), (64, 80)] {
        let started = Instant::now();
        backoff(attempt).await;
        let bounds = Duration::from_millis(base_ms)..=Duration::from_millis(base_ms + 15);
        assert!(bounds.contains(&started.elapsed()));
    }
}

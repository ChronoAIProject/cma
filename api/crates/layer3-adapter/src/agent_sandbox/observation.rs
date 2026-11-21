//! Observe short lifecycle transitions promptly without raising long-running load.
use std::time::Duration;

pub(super) fn refresh_delay(elapsed: Duration) -> Duration {
    if elapsed < Duration::from_secs(30) {
        Duration::from_millis(250)
    } else {
        Duration::from_millis(500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_observation_covers_normal_teardown_and_volume_reattachment() {
        for elapsed in [
            Duration::ZERO,
            Duration::from_secs(8),
            Duration::from_secs(15),
            Duration::from_millis(29_999),
        ] {
            assert_eq!(refresh_delay(elapsed), Duration::from_millis(250));
        }
    }

    #[test]
    fn extended_transitions_return_to_normal_load_at_thirty_seconds() {
        for elapsed in [
            Duration::from_secs(30),
            Duration::from_secs(31),
            Duration::from_secs(120),
            Duration::MAX,
        ] {
            assert_eq!(refresh_delay(elapsed), Duration::from_millis(500));
        }
    }
}

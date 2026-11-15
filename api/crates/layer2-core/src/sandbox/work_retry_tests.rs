use super::*;

#[test]
fn maintenance_retries_back_off_and_keep_pending_probe_work() {
    let base = Duration::from_secs(30);
    for failure in [WorkRetryFailure::Waiting, WorkRetryFailure::Transient] {
        for (attempts, seconds) in [
            (0, 30),
            (1, 60),
            (2, 120),
            (3, 240),
            (4, 300),
            (11, 300),
            (12, 1800),
            (13, 1800),
            (u32::MAX, 1800),
        ] {
            let decision = work_retry_policy(WorkRetryClass::Maintenance, failure, attempts, base);
            assert_eq!(decision.delay.as_secs(), seconds, "{failure:?}/{attempts}");
            assert_eq!(decision.parked, attempts >= 12);
        }
    }
}

#[test]
fn external_action_has_a_small_budget_without_becoming_terminal() {
    for (attempts, seconds, parked) in [
        (0, 30, false),
        (1, 60, false),
        (2, 120, false),
        (3, 1800, true),
        (12, 1800, true),
        (u32::MAX, 1800, true),
    ] {
        assert_eq!(
            work_retry_policy(
                WorkRetryClass::Maintenance,
                WorkRetryFailure::ExternalAction,
                attempts,
                Duration::from_secs(30),
            ),
            WorkRetryDecision {
                delay: Duration::from_secs(seconds),
                parked,
            }
        );
    }
}

#[test]
fn interactive_work_keeps_three_initial_retries_and_never_parks() {
    for failure in [
        WorkRetryFailure::Waiting,
        WorkRetryFailure::Transient,
        WorkRetryFailure::ExternalAction,
    ] {
        for (attempts, seconds) in [
            (0, 30),
            (1, 30),
            (2, 30),
            (3, 60),
            (4, 120),
            (5, 240),
            (6, 300),
            (12, 300),
            (u32::MAX, 300),
        ] {
            let decision = work_retry_policy(
                WorkRetryClass::Interactive,
                failure,
                attempts,
                Duration::from_secs(30),
            );
            assert_eq!(decision.delay.as_secs(), seconds, "{failure:?}/{attempts}");
            assert!(!decision.parked);
        }
    }
}

#[test]
fn configured_base_is_never_shortened_or_overflowed() {
    for class in [WorkRetryClass::Interactive, WorkRetryClass::Maintenance] {
        for base in [
            Duration::from_secs(1),
            Duration::from_secs(600),
            Duration::from_secs(3600),
            Duration::MAX,
        ] {
            for attempts in [0, 1, 2, 3, 11, 12, u32::MAX] {
                let decision =
                    work_retry_policy(class, WorkRetryFailure::Transient, attempts, base);
                assert!(decision.delay >= base);
                if attempts == 0 {
                    assert_eq!(decision.delay, base);
                }
            }
        }
    }
}

#[test]
fn reset_attempts_removes_backoff_without_process_local_state() {
    let decide = |attempts| {
        work_retry_policy(
            WorkRetryClass::Maintenance,
            WorkRetryFailure::Transient,
            attempts,
            Duration::from_secs(30),
        )
    };
    assert!(decide(12).parked);
    assert_eq!(decide(0).delay, Duration::from_secs(30));
    assert!(!decide(0).parked);
}

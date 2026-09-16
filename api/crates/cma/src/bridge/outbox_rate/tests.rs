use super::*;

fn lease(item: &str, expires_at_ms: i64) -> RateLease {
    RateLease {
        item: item.into(),
        attempt: format!("attempt-{item}"),
        expires_at_ms,
    }
}

#[test]
fn active_and_expired_slots_preserve_transport_deadline_and_interval() {
    let mut state = BotRateState::default();
    let first = lease("original", 30_000);
    state.reserve(first.clone(), 1_000, 1_000).unwrap();
    state.require_lease(&first, 29_999).unwrap();
    let retained = state.clone();
    for now in [1_000, 29_999, 30_000, 30_999] {
        assert_eq!(
            state.reserve(lease("later", now + 30_000), now, 1_000),
            Err(RateError::BusyUntil(31_000))
        );
        assert_eq!(state, retained);
    }
    assert_eq!(state.require_lease(&first, 30_000), Err(RateError::Stale));
    state
        .reserve(lease("later", 61_000), 31_000, 1_000)
        .unwrap();
    assert_eq!(state.require_lease(&first, 31_000), Err(RateError::Stale));
}

#[test]
fn completion_and_provider_backoff_delay_new_messages_without_reserving_attempts() {
    let mut state = BotRateState::default();
    let first = lease("first", 31_000);
    state.reserve(first.clone(), 1_000, 1_000).unwrap();
    state.finish(&first, 1_200, 1_000, Some(61_200)).unwrap();
    assert!(state.lease.is_none());
    let retained = state.clone();
    assert_eq!(
        state.reserve(lease("new", 40_000), 2_200, 1_000),
        Err(RateError::BusyUntil(61_200))
    );
    assert_eq!(state, retained);
    let second = lease("new", 91_200);
    state.reserve(second.clone(), 61_200, 1_000).unwrap();
    state.require_lease(&second, 61_200).unwrap();
    assert_eq!(
        state.finish(&first, 61_200, 1_000, None),
        Err(RateError::Stale)
    );
    state.finish(&second, 61_300, 1_000, None).unwrap();
    assert_eq!(state.ready_at(1_000), Ok(62_300));
}

#[test]
fn rate_policy_refuses_invalid_deadlines_and_preserves_long_provider_delays() {
    let mut state = BotRateState::default();
    for interval in [0, -1, 60_001] {
        assert_eq!(state.ready_at(interval), Err(RateError::Invalid));
    }
    assert_eq!(
        state.reserve(lease("x", 90_000), 1_000, 1_000),
        Err(RateError::Invalid)
    );
    let first = lease("first", 31_000);
    state.reserve(first.clone(), 1_000, 1_000).unwrap();
    let retained = state.clone();
    for backoff in [0, 1_001] {
        assert_eq!(
            state.finish(&first, 1_002, 1_000, Some(backoff)),
            Err(RateError::Invalid)
        );
        assert_eq!(state, retained);
    }
    assert_eq!(
        state.require_lease(&lease("wrong", 31_000), 1_001),
        Err(RateError::Stale)
    );
    state.finish(&first, 1_002, 1_000, Some(i64::MAX)).unwrap();
    assert_eq!(state.ready_at(1_000), Ok(i64::MAX));
}

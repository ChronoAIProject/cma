use super::*;
use governor::clock::FakeRelativeClock;
use std::sync::{Arc, Barrier};

type Limits = PublishLimits<FakeRelativeClock>;
fn limits(global: u32, owner: u32) -> (Limits, FakeRelativeClock) {
    let clock = FakeRelativeClock::default();
    let config = AgentEventsConfig {
        rate_per_minute: global,
        owner_rate_per_minute: owner,
        ..Default::default()
    };
    (PublishLimits::with_clock(&config, clock.clone()), clock)
}
fn record(n: usize) -> RecordId {
    RecordId::new(format!("record-{n}")).unwrap()
}
fn consumer(n: usize) -> ConsumerId {
    ConsumerId::parse(&format!("csm_{n:012x}")).unwrap()
}
fn event(n: usize) -> EventId {
    EventId::new(format!("hof_{n:032x}")).unwrap()
}
fn policy(consumer: u16, channel: u16) -> AgentEventPolicy {
    AgentEventPolicy {
        per_consumer_per_minute: consumer,
        per_channel_per_minute: channel,
        ..Default::default()
    }
}
fn charge(
    limits: &Limits,
    record_id: usize,
    publisher: usize,
    policy: &AgentEventPolicy,
) -> Result<(), Duration> {
    limits.check_rates(&record(record_id), "owner", &consumer(publisher), policy)
}

#[test]
fn consumer_policy_refuses_at_its_bound_without_charging_wider_tiers() {
    let (limits, _) = limits(600, 120);
    let policy = policy(3, 60);
    for _ in 0..3 {
        assert_eq!(charge(&limits, 0, 0, &policy), Ok(()));
    }
    assert_eq!(charge(&limits, 0, 0, &policy), Err(WINDOW));
    for _ in 0..597 {
        assert!(limits.global.check().is_ok());
    }
    assert!(limits.global.check().is_err());
}

#[test]
fn fixed_consumer_ceiling_precedes_its_policy_window() {
    let (limits, _) = limits(600, 600);
    let policy = policy(30, 60);
    for _ in 0..30 {
        charge(&limits, 0, 0, &policy).unwrap();
    }
    assert_eq!(charge(&limits, 0, 0, &policy), Err(Duration::from_secs(2)));
}

#[test]
fn channel_ceiling_and_policy_use_the_lower_bound() {
    for (global, bound, expected) in [(100, 60, 10), (600, 7, 7)] {
        let (limits, _) = limits(global, global);
        let policy = policy(7, bound);
        for n in 0..expected {
            charge(&limits, 0, n % 2, &policy).unwrap();
        }
        let retry = charge(&limits, 0, expected % 2, &policy).unwrap_err();
        assert_eq!(
            retry,
            if global == 100 {
                Duration::from_secs(6)
            } else {
                WINDOW
            }
        );
        assert_eq!(
            limits.channel_windows.get(&record(0)).unwrap().used,
            expected as u32
        );
    }
}

#[test]
fn record_aliases_share_consumer_and_channel_allowances() {
    let (limits, _) = limits(600, 120);
    let policy = policy(1, 2);
    let aliases = [
        ("old-sealed-channel", record(0)),
        ("new-sealed-channel", record(0)),
    ];
    limits
        .check_rates(&aliases[0].1, "owner", &consumer(0), &policy)
        .unwrap();
    assert_eq!(
        limits.check_rates(&aliases[1].1, "owner", &consumer(0), &policy),
        Err(WINDOW)
    );
    limits
        .check_rates(&aliases[1].1, "owner", &consumer(1), &policy)
        .unwrap();
    assert_eq!(
        limits.check_rates(&aliases[0].1, "owner", &consumer(2), &policy),
        Err(WINDOW)
    );
    assert_eq!(limits.channel_windows.len(), 1);
}

#[test]
fn owner_budget_spans_records_and_global_spans_owners() {
    let (limits, _) = limits(600, 5);
    for n in 0..5 {
        charge(&limits, n % 2, 0, &policy(30, 60)).unwrap();
    }
    assert_eq!(
        charge(&limits, 2, 0, &policy(30, 60)),
        Err(Duration::from_secs(12))
    );
    assert!(limits
        .check_rates(&record(3), "other", &consumer(0), &policy(30, 60))
        .is_ok());
    let (limits, _) = self::limits(100, 100);
    for n in 0..100 {
        limits
            .check_rates(
                &record(n),
                &format!("owner-{n}"),
                &consumer(0),
                &policy(30, 60),
            )
            .unwrap();
    }
    assert_eq!(
        limits.check_rates(&record(100), "new-owner", &consumer(0), &policy(30, 60)),
        Err(Duration::from_millis(600))
    );
}

#[test]
fn policy_edits_and_denials_do_not_reset_the_window() {
    for consumer_policy in [true, false] {
        let (limits, clock) = limits(600, 120);
        let active = |n| {
            if consumer_policy {
                policy(n, 60)
            } else {
                policy(1, n)
            }
        };
        let mut publisher = 0;
        let mut attempt = |bound| {
            // Distinct publishers isolate the record window while each
            // stored consumer policy remains <= its channel policy.
            if !consumer_policy {
                publisher += 1;
            }
            charge(&limits, 0, publisher, &active(bound))
        };
        for _ in 0..2 {
            attempt(3).unwrap();
        }
        assert_eq!(attempt(1), Err(WINDOW));
        for _ in 0..2 {
            attempt(4).unwrap();
        }
        assert_eq!(attempt(4), Err(WINDOW));
        clock.advance(Duration::from_secs(30));
        assert_eq!(attempt(4), Err(Duration::from_secs(30)));
        clock.advance(Duration::from_secs(30));
        assert_eq!(attempt(1), Ok(()));
    }
}

#[test]
fn maintenance_removes_idle_governor_keys_and_policy_windows() {
    let (limits, clock) = limits(600, 120);
    charge(&limits, 0, 0, &policy(30, 60)).unwrap();
    clock.advance(Duration::from_secs(61));
    // Housekeeping runs on the 256th request, before checking its tiers.
    limits.calls.store(255, Ordering::Relaxed);
    charge(&limits, 1, 1, &policy(30, 60)).unwrap();
    assert_eq!(limits.per_consumer.len(), 1);
    assert_eq!(limits.per_channel.len(), 1);
    assert_eq!(limits.per_owner.len(), 1);
    assert_eq!(limits.consumer_windows.len(), 1);
    assert_eq!(limits.channel_windows.len(), 1);
    assert!(!limits
        .consumer_windows
        .contains_key(&(record(0), consumer(0))));
    assert!(!limits.channel_windows.contains_key(&record(0)));
    clock.advance(Duration::from_secs(61));
    limits.maintain();
    assert!(limits.per_consumer.is_empty());
    assert!(limits.per_channel.is_empty());
    assert!(limits.per_owner.is_empty());
    assert!(limits.consumer_windows.is_empty());
    assert!(limits.channel_windows.is_empty());
}

#[test]
fn roots_are_record_scoped_and_allow_retries_after_sixteen_ids() {
    let (limits, _) = limits(600, 120);
    let root = RootId::derive(&record(0), &EventId::new("producer-id").unwrap());
    for n in 0..16 {
        limits.admit_root(&record(0), &root, &event(n)).unwrap();
    }
    assert_eq!(
        limits.admit_root(&record(0), &root, &event(16)),
        Err(RootLimit::EventCount)
    );
    assert_eq!(limits.admit_root(&record(0), &root, &event(0)), Ok(()));
    assert_eq!(limits.admit_root(&record(1), &root, &event(16)), Ok(()));
    let independent = RootId::derive(&record(1), &EventId::new("producer-id").unwrap());
    assert_ne!(root, independent);
}

#[test]
fn root_capacity_fails_closed_and_retries_never_refresh_expiry() {
    let (mut limits, clock) = limits(600, 120);
    limits.root_capacity = 1;
    let root = RootId::derive(&record(0), &event(0));
    let other = RootId::derive(&record(0), &event(1));
    for n in 0..16 {
        limits.admit_root(&record(0), &root, &event(n)).unwrap();
    }
    assert_eq!(
        limits.admit_root(&record(0), &other, &event(0)),
        Err(RootLimit::Capacity)
    );
    clock.advance(ROOT_TTL - Duration::from_secs(1));
    limits.admit_root(&record(0), &root, &event(0)).unwrap();
    clock.advance(Duration::from_secs(1));
    // Expiry precedes the existing-root branch as well as a new-root insert.
    limits.admit_root(&record(0), &root, &event(16)).unwrap();
    assert_eq!(
        limits
            .roots
            .get(&(record(0), root.as_str().into()))
            .unwrap()
            .published
            .len(),
        1
    );
    clock.advance(ROOT_TTL);
    limits.admit_root(&record(0), &other, &event(0)).unwrap();
    assert_eq!(limits.roots.len(), 1);
    assert!(!limits
        .roots
        .contains_key(&(record(0), root.as_str().into())));
}

#[test]
fn concurrent_new_roots_cannot_overrun_capacity() {
    let (mut limits, _) = limits(600, 120);
    limits.root_capacity = 1;
    let limits = Arc::new(limits);
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let limits = limits.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let root = RootId::derive(&record(n), &event(0));
                barrier.wait();
                limits.admit_root(&record(n), &root, &event(0))
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(RootLimit::Capacity))
            .count(),
        7
    );
    assert_eq!(limits.roots.len(), 1);
}

//! Pod-wide byte and job budgets, and the per-channel limits.
use cmaeg::media::{
    budget::{ChannelLimiter, InflightBytes, JobPermits, KeyedQuota, ResponseBudget},
    MediaLimits,
};
use cmaeg_core::ids::ChannelId;
use std::{num::NonZeroU32, time::Duration};
use tokio::time::Instant;

pub(crate) fn limits() -> MediaLimits {
    MediaLimits {
        max_bytes: 1024 * 1024,
        inflight_bytes: 64 * 1024,
        response_bytes: 64 * 1024,
        max_jobs: 2,
        cache_bytes: 0,
        fetch_per_minute: 60,
        download_bytes_per_hour: 20 * 1024 * 1024,
    }
}

pub(crate) fn channel(name: &str) -> ChannelId {
    ChannelId::new(format!("cmaeg1.ch.{name}")).unwrap()
}

#[tokio::test(start_paused = true)]
async fn an_exhausted_response_budget_refuses_at_the_deadline() {
    let budget = ResponseBudget::new(&limits());
    let held = budget
        .acquire(64 * 1024, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(budget.available_bytes(), 0);
    let started = Instant::now();
    let refused = budget.acquire(1, started + Duration::from_secs(2)).await;
    assert!(refused.is_none());
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    drop(held);
    assert_eq!(budget.available_bytes(), 64 * 1024);
    assert!(budget
        .acquire(1, Instant::now() + Duration::from_secs(1))
        .await
        .is_some());
}

#[tokio::test(start_paused = true)]
async fn a_response_larger_than_the_budget_is_refused_immediately() {
    let budget = ResponseBudget::new(&limits());
    let started = Instant::now();
    assert!(budget
        .acquire(64 * 1024 + 1, started + Duration::from_secs(60))
        .await
        .is_none());
    assert_eq!(Instant::now(), started);
    assert_eq!(budget.available_bytes(), 64 * 1024);
}

#[tokio::test(start_paused = true)]
async fn inflight_bytes_reserve_at_most_the_whole_budget() {
    let budget = InflightBytes::new(&limits());
    let deadline = Instant::now() + Duration::from_secs(1);
    let small = budget.acquire(1_000, deadline).await.unwrap();
    // Rounded up to whole KiB, never down.
    assert_eq!(budget.available_bytes(), 63 * 1024);
    drop(small);
    let whole = budget.acquire(10 * 1024 * 1024, deadline).await.unwrap();
    assert_eq!(budget.available_bytes(), 0);
    assert!(budget
        .acquire(1, Instant::now() + Duration::from_millis(10))
        .await
        .is_none());
    drop(whole);
    assert_eq!(budget.available_bytes(), 64 * 1024);
}

#[tokio::test(start_paused = true)]
async fn job_permits_count_running_jobs() {
    let jobs = JobPermits::new(&limits());
    let deadline = Instant::now() + Duration::from_secs(1);
    let first = jobs.acquire(deadline).await.unwrap();
    let second = jobs.acquire(deadline).await.unwrap();
    assert_eq!(jobs.available(), 0);
    assert!(jobs.acquire(deadline).await.is_none());
    drop(first);
    assert_eq!(jobs.available(), 1);
    drop(second);
    assert_eq!(jobs.available(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_third_slot_on_one_channel_waits_while_another_channel_proceeds() {
    let limiter = std::sync::Arc::new(ChannelLimiter::new(&limits()));
    let a = channel("a");
    let deadline = Instant::now() + Duration::from_secs(60);
    let first = limiter.derive_slot(&a, deadline).await.unwrap();
    let _second = limiter.derive_slot(&a, deadline).await.unwrap();
    let waiting = {
        let limiter = limiter.clone();
        let a = a.clone();
        tokio::spawn(async move { limiter.derive_slot(&a, deadline).await })
    };
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!waiting.is_finished());
    // Another channel is unaffected by A's queue.
    assert!(limiter
        .derive_slot(&channel("b"), Instant::now() + Duration::from_millis(1))
        .await
        .is_some());
    drop(first);
    assert!(waiting.await.unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn a_queued_slot_gives_up_at_its_deadline() {
    let limiter = ChannelLimiter::new(&limits());
    let a = channel("a");
    let deadline = Instant::now() + Duration::from_secs(60);
    let _first = limiter.derive_slot(&a, deadline).await.unwrap();
    let _second = limiter.derive_slot(&a, deadline).await.unwrap();
    assert!(limiter
        .derive_slot(&a, Instant::now() + Duration::from_secs(1))
        .await
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn the_sweep_removes_idle_slot_entries_but_keeps_busy_ones() {
    let limiter = ChannelLimiter::new(&limits());
    let deadline = Instant::now() + Duration::from_secs(1);
    let busy = limiter
        .derive_slot(&channel("busy"), deadline)
        .await
        .unwrap();
    for index in 0..600 {
        drop(
            limiter
                .derive_slot(&channel(&format!("idle-{index}")), deadline)
                .await
                .unwrap(),
        );
    }
    // Calls 256 and 512 swept every idle entry before them.
    assert!(limiter.slots.len() < 100, "{}", limiter.slots.len());
    assert!(limiter.slots.contains_key(&channel("busy")));
    drop(busy);
}

#[test]
fn keyed_quotas_refuse_past_their_burst() {
    let quota = KeyedQuota::per_minute(NonZeroU32::new(3).unwrap());
    let a = channel("a");
    assert!(quota.check(&a));
    assert!(quota.check_n(&a, 2));
    assert!(!quota.check(&a));
    // Another key has its own burst.
    assert!(quota.check(&channel("b")));
    // A batch above the burst can never conform.
    let hourly = KeyedQuota::per_hour(NonZeroU32::new(10).unwrap());
    assert!(!hourly.check_n(&a, 11));
    assert!(hourly.check_n(&a, 10));
    assert!(hourly.check_n(&a, 0));
}

#[test]
fn keyed_quotas_stay_bounded_after_ten_thousand_keys() {
    // 6000 per minute replenishes one cell every 10 ms.
    let quota = KeyedQuota::per_minute(NonZeroU32::new(6_000).unwrap());
    for index in 0..10_000 {
        assert!(quota.check(&channel(&format!("key-{index}"))));
    }
    std::thread::sleep(Duration::from_millis(50));
    let repeat = channel("repeat");
    for _ in 0..256 {
        quota.check(&repeat);
    }
    assert!(quota.len() < 300, "{}", quota.len());
    assert!(!quota.is_empty());
}

#[test]
fn channel_limiters_bound_their_keyed_quotas() {
    let limiter = ChannelLimiter::new(&limits());
    let a = channel("a");
    for _ in 0..60 {
        assert!(limiter.fetch.check(&a));
    }
    assert!(!limiter.fetch.check(&a));
    for _ in 0..30 {
        assert!(limiter.link.check(&a));
    }
    assert!(!limiter.link.check(&a));
    // 20 MiB per hour is 20480 KiB.
    assert!(limiter.download_kib.check_n(&a, 20_480));
    assert!(!limiter.download_kib.check_n(&a, 1));
}

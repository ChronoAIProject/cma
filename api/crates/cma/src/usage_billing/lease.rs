//! Local cancellation complements the provider's durable transaction-ID dedupe.
use super::*;
use crate::store::sandbox_usage::billing::DELIVERY_LEASE_SECONDS;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::{Instant, MissedTickBehavior};

const RENEW_INTERVAL: Duration = Duration::from_secs(30);
const STORE_DEADLINE: Duration = Duration::from_secs(5);
const LEASE_DURATION: Duration = Duration::from_secs(DELIVERY_LEASE_SECONDS as u64);

pub(super) struct Clock {
    wall: i64,
    started: Instant,
}
impl Clock {
    fn new() -> Self {
        Self {
            wall: now(),
            started: Instant::now(),
        }
    }
    pub(super) fn now(&self) -> i64 {
        self.wall
            .saturating_add(self.started.elapsed().as_secs() as i64)
    }
}

impl Exporter {
    pub(super) async fn deliver(&self, delivery: &Delivery) -> Result<(), &'static str> {
        self.deliver_guarded(delivery, RENEW_INTERVAL).await
    }

    pub(super) async fn deliver_guarded(
        &self,
        delivery: &Delivery,
        renew_interval: Duration,
    ) -> Result<(), &'static str> {
        let clock = Clock::new();
        let mut valid_until = self.renew_delivery(delivery, &clock).await?;
        let post_started = AtomicBool::new(false);
        let work = self.deliver_claimed(delivery, &clock, &post_started);
        tokio::pin!(work);
        let mut heartbeat =
            tokio::time::interval_at(Instant::now() + renew_interval, renew_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let lost = || {
            if post_started.load(Ordering::Relaxed) {
                "provider_outcome_unknown"
            } else {
                "billing_claim_lost"
            }
        };
        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(valid_until) => return Err(lost()),
                _ = heartbeat.tick() => {
                    valid_until = tokio::time::timeout_at(valid_until, self.renew_delivery(delivery, &clock))
                        .await.map_err(|_| lost())?.map_err(|_| lost())?;
                }
                result = &mut work => return result,
            }
        }
    }

    async fn renew_delivery(
        &self,
        delivery: &Delivery,
        clock: &Clock,
    ) -> Result<Instant, &'static str> {
        // Start the monotonic allowance BEFORE IO. A slow acknowledgement or
        // wall-clock rollback cannot extend this attempt's authority locally.
        let started = Instant::now();
        tokio::time::timeout(
            STORE_DEADLINE,
            self.store.renew_usage_delivery(delivery, clock.now()),
        )
        .await
        .map_err(|_| "billing_claim_lost")?
        .map_err(|_| "billing_claim_lost")?;
        Ok(started + LEASE_DURATION - Duration::from_secs(1))
    }
}

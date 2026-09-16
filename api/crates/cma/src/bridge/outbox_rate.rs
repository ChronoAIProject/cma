//! Durable send pacing policy. The store serializes reservations for the same
//! provider/platform/bot across Bridge configurations, keys and API replicas.
//! This module does not grant delivery authority or make an Unknown retryable.

const MAX_LEASE_MS: i64 = 60_000;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RateLease {
    pub item: String,
    pub attempt: String,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BotRateState {
    pub next_start_ms: i64,
    pub cooldown_until_ms: i64,
    pub lease: Option<RateLease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RateError {
    Invalid,
    BusyUntil(i64),
    Stale,
}

impl BotRateState {
    /// A crashed worker keeps its slot through the original transport deadline
    /// plus the minimum interval. A replacement cannot overlap an old send.
    pub fn ready_at(&self, interval_ms: i64) -> Result<i64, RateError> {
        if !(1..=60_000).contains(&interval_ms)
            || self.next_start_ms < 0
            || self.cooldown_until_ms < 0
        {
            return Err(RateError::Invalid);
        }
        let lease_deadline = match &self.lease {
            Some(lease) => {
                validate_lease(lease)?;
                lease
                    .expires_at_ms
                    .checked_add(interval_ms)
                    .ok_or(RateError::Invalid)?
            }
            None => 0,
        };
        Ok(self
            .next_start_ms
            .max(self.cooldown_until_ms)
            .max(lease_deadline))
    }

    /// Call in the same transaction that writes the original outbox claim.
    /// Busy leaves the outbox unclaimed and consumes no delivery attempt.
    pub fn reserve(
        &mut self,
        lease: RateLease,
        now_ms: i64,
        interval_ms: i64,
    ) -> Result<(), RateError> {
        validate_lease(&lease)?;
        if now_ms < 0
            || lease.expires_at_ms <= now_ms
            || lease.expires_at_ms - now_ms > MAX_LEASE_MS
        {
            return Err(RateError::Invalid);
        }
        let ready = self.ready_at(interval_ms)?;
        if now_ms < ready {
            return Err(RateError::BusyUntil(ready));
        }
        self.lease = Some(lease);
        Ok(())
    }

    /// Recheck alongside exact Sending state immediately before transport IO.
    pub fn require_lease(&self, expected: &RateLease, now_ms: i64) -> Result<(), RateError> {
        validate_lease(expected)?;
        if self.lease.as_ref() != Some(expected)
            || now_ms < 0
            || now_ms >= expected.expires_at_ms
            || now_ms < self.next_start_ms
            || now_ms < self.cooldown_until_ms
        {
            return Err(RateError::Stale);
        }
        Ok(())
    }

    /// Finish atomically with the observed outbox outcome. Retry-After paces
    /// subsequent sends; it never establishes that this attempt was not sent.
    pub fn finish(
        &mut self,
        expected: &RateLease,
        now_ms: i64,
        interval_ms: i64,
        provider_retry_at_ms: Option<i64>,
    ) -> Result<(), RateError> {
        validate_lease(expected)?;
        self.ready_at(interval_ms)?;
        if self.lease.as_ref() != Some(expected) || now_ms < 0 {
            return Err(RateError::Stale);
        }
        if provider_retry_at_ms.is_some_and(|at| at <= now_ms) {
            return Err(RateError::Invalid);
        }
        let next = now_ms.checked_add(interval_ms).ok_or(RateError::Invalid)?;
        self.next_start_ms = self.next_start_ms.max(next);
        self.cooldown_until_ms = self
            .cooldown_until_ms
            .max(provider_retry_at_ms.unwrap_or(0));
        self.lease = None;
        Ok(())
    }
}

fn validate_lease(lease: &RateLease) -> Result<(), RateError> {
    if lease.expires_at_ms <= 0
        || [&lease.item, &lease.attempt]
            .iter()
            .any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
    {
        return Err(RateError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
#[path = "outbox_rate/tests.rs"]
mod tests;

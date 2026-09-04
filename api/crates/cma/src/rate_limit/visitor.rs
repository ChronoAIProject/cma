//! Daily visitor Launch quota, separate from HTTP request-rate budgets.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use layer2_core::SubjectId;

use super::{retry_after_seconds, RateLimiters};

const UTC_DAY_SECONDS: u64 = 24 * 60 * 60;
const VISITOR_QUOTA_KEY_PREFIX: &str = "trigger-visitor:";

impl RateLimiters {
    /// Charge one newly-created visitor Launch against the current daily limit.
    ///
    /// The launch command calls this only after idempotency has distinguished a
    /// new aggregate from an exact replay. The limit is supplied per call
    /// because platform settings are live.
    #[allow(dead_code)] // Wired by the post-refactor Launch service from #821.
    pub(crate) fn admit_visitor_launch(
        &self,
        subject: &SubjectId,
        limit: u32,
        now: SystemTime,
    ) -> Result<(), ResourceQuotaExceeded> {
        self.visitor_launches.admit(subject, limit, now)
    }
}

/// The resource whose count has reached a configured boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuotaResource {
    VisitorLaunches,
}

impl QuotaResource {
    #[allow(dead_code)] // Consumed by the #821 HTTP problem projection.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::VisitorLaunches => "visitor_launches",
        }
    }
}

/// Typed information needed to render a `resource_quota` refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceQuotaExceeded {
    pub(crate) resource: QuotaResource,
    pub(crate) limit: u32,
    pub(crate) reset_at: SystemTime,
}

impl ResourceQuotaExceeded {
    #[allow(dead_code)] // Consumed by the #821 HTTP problem projection.
    pub(crate) const fn code(&self) -> &'static str {
        "resource_quota"
    }

    #[allow(dead_code)] // Consumed by the #821 HTTP problem projection.
    pub(crate) fn retry_after_seconds(&self, now: SystemTime) -> u64 {
        self.reset_at
            .duration_since(now)
            .map(retry_after_seconds)
            .unwrap_or(1)
    }
}

#[derive(Default)]
pub(super) struct VisitorLaunchQuota {
    usage: Mutex<VisitorDayUsage>,
}

#[derive(Default)]
struct VisitorDayUsage {
    utc_day: u64,
    launches: HashMap<String, u32>,
}

impl VisitorLaunchQuota {
    fn admit(
        &self,
        subject: &SubjectId,
        limit: u32,
        now: SystemTime,
    ) -> Result<(), ResourceQuotaExceeded> {
        let (utc_day, reset_at) = utc_day_window(now);
        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if usage.utc_day != utc_day {
            usage.utc_day = utc_day;
            usage.launches.clear();
        }

        let used = usage
            .launches
            .entry(visitor_quota_key(subject))
            .or_default();
        if *used >= limit {
            return Err(ResourceQuotaExceeded {
                resource: QuotaResource::VisitorLaunches,
                limit,
                reset_at,
            });
        }
        *used += 1;
        Ok(())
    }
}

pub(crate) fn visitor_quota_key(subject: &SubjectId) -> String {
    format!("{VISITOR_QUOTA_KEY_PREFIX}{}", subject.as_str())
}

fn utc_day_window(now: SystemTime) -> (u64, SystemTime) {
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let utc_day = elapsed.as_secs() / UTC_DAY_SECONDS;
    let reset_seconds = utc_day.saturating_add(1).saturating_mul(UTC_DAY_SECONDS);
    let reset_at = UNIX_EPOCH
        .checked_add(Duration::from_secs(reset_seconds))
        .unwrap_or(now);
    (utc_day, reset_at)
}

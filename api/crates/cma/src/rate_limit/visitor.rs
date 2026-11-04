//! Daily visitor Launch quota, separate from HTTP request-rate budgets.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use layer1_public::rest::{LaunchQuota, LaunchQuotaError, LaunchQuotaPermit, LaunchQuotaRefusal};
use layer2_core::{Caller, Capability, PlatformSettingsService, SubjectId};

use super::RateLimiters;

const UTC_DAY_SECONDS: u64 = 24 * 60 * 60;
const VISITOR_QUOTA_KEY_PREFIX: &str = "trigger-visitor:";

/// CMA's live-settings adapter for the public Launch quota boundary.
#[allow(dead_code)] // Constructed by the Trigger route composition that consumes this slice.
pub(crate) struct CmaLaunchQuota {
    limiters: Arc<RateLimiters>,
    settings: Arc<PlatformSettingsService>,
    clock: Arc<dyn Fn() -> SystemTime + Send + Sync>,
}

impl CmaLaunchQuota {
    #[allow(dead_code)] // Constructed by the Trigger route composition that consumes this slice.
    pub(crate) fn new(limiters: Arc<RateLimiters>, settings: Arc<PlatformSettingsService>) -> Self {
        Self {
            limiters,
            settings,
            clock: Arc::new(SystemTime::now),
        }
    }

    #[cfg(test)]
    pub(super) fn with_clock(
        limiters: Arc<RateLimiters>,
        settings: Arc<PlatformSettingsService>,
        clock: impl Fn() -> SystemTime + Send + Sync + 'static,
    ) -> Self {
        Self {
            limiters,
            settings,
            clock: Arc::new(clock),
        }
    }
}

#[async_trait::async_trait]
impl LaunchQuota for CmaLaunchQuota {
    async fn reserve(&self, caller: &Caller) -> Result<LaunchQuotaPermit, LaunchQuotaError> {
        // Production admits the daily quota in the same durable transaction as
        // the Launch. A cancelled response cannot lose or release that charge.
        if self.limiters.shared.is_some() {
            return Ok(LaunchQuotaPermit::unmetered());
        }
        if !is_visitor(caller) {
            return Ok(LaunchQuotaPermit::unmetered());
        }
        let settings = self.settings.current().await.map_err(|error| {
            tracing::error!(error = %error, "could not read visitor Launch quota");
            LaunchQuotaError::Unavailable
        })?;
        let reservation = self
            .limiters
            .admit_visitor_launch(
                &caller.subject,
                settings.settings.limits.visitor_max_launches_per_day,
                (self.clock)(),
            )
            .map_err(LaunchQuotaError::ResourceQuota)?;
        Ok(LaunchQuotaPermit::reserved(move |created| {
            if created {
                reservation.commit();
            }
        }))
    }
}

fn is_visitor(caller: &Caller) -> bool {
    caller.authorization.allows(Capability::LaunchTriggers)
        && !caller.authorization.allows(Capability::CreateSandbox)
}

impl RateLimiters {
    /// Atomically reserve one newly-created visitor Launch against a live limit.
    pub(crate) fn admit_visitor_launch(
        self: &Arc<Self>,
        subject: &SubjectId,
        limit: u32,
        now: SystemTime,
    ) -> Result<VisitorLaunchReservation, LaunchQuotaRefusal> {
        let (utc_day, reset_at) = utc_day_window(now);
        let key = visitor_quota_key(subject);
        self.visitor_launches
            .reserve(&key, utc_day, limit, reset_at)?;
        Ok(VisitorLaunchReservation {
            limiters: Arc::clone(self),
            key,
            utc_day,
            active: true,
        })
    }
}

pub(crate) struct VisitorLaunchReservation {
    limiters: Arc<RateLimiters>,
    key: String,
    utc_day: u64,
    active: bool,
}

impl VisitorLaunchReservation {
    pub(crate) fn commit(mut self) {
        self.active = false;
    }
}

impl Drop for VisitorLaunchReservation {
    fn drop(&mut self) {
        if self.active {
            self.limiters
                .visitor_launches
                .release(&self.key, self.utc_day);
        }
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
    fn reserve(
        &self,
        key: &str,
        utc_day: u64,
        limit: u32,
        reset_at: SystemTime,
    ) -> Result<(), LaunchQuotaRefusal> {
        let mut usage = self.lock();
        reset_day(&mut usage, utc_day);
        let used = usage.launches.entry(key.to_owned()).or_default();
        if *used >= limit {
            return Err(LaunchQuotaRefusal::visitor_launches(limit, reset_at));
        }
        *used += 1;
        Ok(())
    }

    fn release(&self, key: &str, utc_day: u64) {
        let mut usage = self.lock();
        if usage.utc_day != utc_day {
            return;
        }
        let remove = usage.launches.get_mut(key).is_some_and(|used| {
            debug_assert!(*used > 0, "a visitor reservation is released at most once");
            *used = used.saturating_sub(1);
            *used == 0
        });
        if remove {
            usage.launches.remove(key);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VisitorDayUsage> {
        self.usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn reset_day(usage: &mut VisitorDayUsage, utc_day: u64) {
    if usage.utc_day != utc_day {
        usage.utc_day = utc_day;
        usage.launches.clear();
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

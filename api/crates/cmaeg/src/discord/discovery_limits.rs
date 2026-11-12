//! Discord quota observations are not permanent source configuration.
use super::{
    discovery::{Discovery, SessionLimit},
    status::{FailureCode, StatusAttempt},
    GatewayError,
};

pub(super) fn validate(discovery: &Discovery, status: &StatusAttempt) -> Result<(), GatewayError> {
    let limit = &discovery.session_start_limit;
    // https://docs.discord.com/developers/events/gateway#session-start-limit-object
    // reset_after is milliseconds, with no positive minimum or seven-day cap.
    // Preserve diagnostics for those former guards without refusing valid time
    // boundaries. Numeric fields are bounded u64s; never log the response/URL.
    let checks = [
        (limit.total == 0, "total_zero", true),
        (limit.remaining > limit.total, "remaining_gt_total", true),
        (limit.max_concurrency == 0, "max_concurrency_zero", true),
        (limit.reset_after == 0, "reset_after_zero", false),
        (
            limit.reset_after > 7 * 86_400_000,
            "reset_after_gt_7d",
            false,
        ),
    ];
    let mut refused = false;
    for (matches, reason, rejects) in checks {
        if !matches {
            continue;
        }
        refused |= rejects;
        if status.allow_diagnostic() {
            tracing::info!(parent: None, stage = "discovery", reason, refused = rejects,
                shards = discovery.shards, total = limit.total, remaining = limit.remaining,
                reset_after = limit.reset_after, max_concurrency = limit.max_concurrency,
                "native Discord session limit observed");
        }
    }
    if refused {
        status.failure(FailureCode::SessionLimitInvalid);
        Err(GatewayError::SessionLimitUnavailable)
    } else {
        Ok(())
    }
}

impl SessionLimit {
    pub(super) fn reset_deadline(&self, now: i64) -> i64 {
        if self.reset_after == 0 {
            // Unknown future reset: preserve the local consumed budget until a
            // positive observation establishes a window, never mint Identify.
            i64::MAX
        } else {
            now.saturating_add(i64::try_from(self.reset_after).unwrap_or(i64::MAX))
        }
    }
}

#[cfg(test)]
#[path = "discovery_limits_tests.rs"]
mod tests;

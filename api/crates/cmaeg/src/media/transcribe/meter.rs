//! What a transcription costs and who may still spend.
//!
//! A direct upload is metered from its size alone: declared durations are
//! sender-defined or absent, so they never decide what is charged. Three
//! limits then bound spend: a per-channel call rate, a per-channel hourly
//! budget of metered seconds and a global hourly backstop.
use crate::{media::budget::at_least_one, media::budget::KeyedQuota, telemetry, SttConfig};
use cmaeg_core::ids::ChannelId;
use dashmap::DashMap;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::time::Instant;

/// 32 kbps is below Telegram and Lark voice bitrates, so bytes / 4000 bounds the real duration
/// of such audio from above without trusting declared values or parsing containers (the
/// established parser is MPL-2.0, outside deny.toml). Opus below 32 kbps is under-metered;
/// see Operations "Known limit" and probe P3.
pub const METERING_FLOOR_BYTES_PER_SECOND: u64 = 4000;
/// The at-budget record is swept, and its gauge refreshed, once every this
/// many charges, on the same cadence as the keyed limiters' own sweep.
const SWEEP_EVERY: u64 = 256;
/// How long a channel refused by its own budget counts as at budget.
const AT_BUDGET_WINDOW: Duration = Duration::from_secs(3_600);

/// The largest original sent as it is: `min(max_bytes, max_seconds × 4000)`.
///
/// Anything up to it meters at most `max_seconds`, so a direct upload can
/// never cost more than one call is allowed to.
pub fn direct_cap(max_bytes: u64, max_seconds: u32) -> u64 {
    max_bytes.min(u64::from(max_seconds).saturating_mul(METERING_FLOOR_BYTES_PER_SECOND))
}

/// The seconds a direct upload of `bytes` is charged: `ceil(bytes / 4000)`,
/// at least one, saturating.
pub fn direct_seconds(bytes: u64) -> u32 {
    u32::try_from(bytes.div_ceil(METERING_FLOOR_BYTES_PER_SECOND))
        .unwrap_or(u32::MAX)
        .max(1)
}

/// Why a transcription was not admitted. Every refusal reaches the agent as
/// `rate_limited`; the outcome tells operators which limit refused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendRefusal {
    /// The channel's calls per minute.
    CallRate,
    /// The channel's metered seconds per hour.
    ChannelBudget,
    /// The pod's metered seconds per hour.
    GlobalBudget,
}

impl SpendRefusal {
    /// The `outcome` label of `cmaeg_media_items_total`.
    pub fn outcome(&self) -> &'static str {
        match self {
            Self::CallRate => "stt_rate_limited",
            Self::ChannelBudget => "stt_channel_budget_exhausted",
            Self::GlobalBudget => "stt_budget_exhausted",
        }
    }
}

/// The pod's speech-to-text spend limits.
pub struct SttSpend {
    calls: KeyedQuota,
    channel_seconds: KeyedQuota,
    global_seconds: DefaultDirectRateLimiter,
    /// When each channel was last refused by its own budget.
    at_budget: DashMap<ChannelId, Instant>,
    checks: AtomicU64,
}

impl SttSpend {
    pub fn new(config: &SttConfig) -> Self {
        Self {
            calls: KeyedQuota::per_minute(at_least_one(config.rate_per_minute)),
            channel_seconds: KeyedQuota::per_hour(at_least_one(config.channel_seconds_per_hour)),
            global_seconds: RateLimiter::direct(Quota::per_hour(at_least_one(
                config.seconds_per_hour,
            ))),
            at_budget: DashMap::new(),
            checks: AtomicU64::new(0),
        }
    }

    /// Take one of the channel's calls for this minute.
    pub fn admit_call(&self, channel: &ChannelId) -> Result<(), SpendRefusal> {
        if self.calls.check(channel) {
            Ok(())
        } else {
            Err(SpendRefusal::CallRate)
        }
    }

    /// Charge `seconds` to the channel's budget, then to the global one.
    ///
    /// The channel goes first so that one channel's senders exhaust only that
    /// channel and never the global budget everyone shares. A global refusal
    /// after the channel was charged leaves the channel charged: that only
    /// errs towards spending less.
    pub fn charge(&self, channel: &ChannelId, seconds: u32) -> Result<(), SpendRefusal> {
        if self
            .checks
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY)
        {
            self.sweep();
        }
        if !self.channel_seconds.check_n(channel, seconds) {
            self.at_budget.insert(channel.clone(), Instant::now());
            self.publish();
            tracing::warn!(
                channel = %telemetry::channel_hash(channel.as_str()),
                seconds,
                "speech-to-text channel budget exhausted"
            );
            return Err(SpendRefusal::ChannelBudget);
        }
        if !take_global(&self.global_seconds, seconds) {
            tracing::warn!(seconds, "speech-to-text global budget exhausted");
            return Err(SpendRefusal::GlobalBudget);
        }
        Ok(())
    }

    /// The seconds a finished call counts: the provider's reported usage when
    /// it exceeds what was metered, otherwise what was metered.
    ///
    /// The excess is charged to both budgets on a best-effort basis: the call
    /// already happened, so a refusal cannot undo it and is ignored, and the
    /// budgets catch up on the next call instead.
    pub fn reconcile(&self, channel: &ChannelId, metered: u32, usage: Option<u32>) -> u32 {
        let Some(usage) = usage.filter(|usage| *usage > metered) else {
            return metered;
        };
        let excess = usage - metered;
        let channel_charged = self.channel_seconds.check_n(channel, excess);
        let global_charged = take_global(&self.global_seconds, excess);
        tracing::debug!(
            channel = %telemetry::channel_hash(channel.as_str()),
            metered,
            usage,
            channel_charged,
            global_charged,
            "speech-to-text usage exceeded the metered seconds"
        );
        usage
    }

    /// Channels refused by their own budget within the last hour.
    pub fn channels_at_budget(&self) -> usize {
        self.at_budget
            .iter()
            .filter(|entry| entry.value().elapsed() < AT_BUDGET_WINDOW)
            .count()
    }

    /// Forget channels whose last refusal is an hour old, then publish.
    fn sweep(&self) {
        self.at_budget
            .retain(|_, refused| refused.elapsed() < AT_BUDGET_WINDOW);
        self.publish();
    }

    fn publish(&self) {
        metrics::gauge!(telemetry::names::STT_CHANNELS_AT_BUDGET)
            .set(self.channels_at_budget() as f64);
    }
}

/// Take `seconds` cells from the global budget at once, or none. A batch
/// larger than the whole budget can never conform and is refused like one
/// that must wait.
fn take_global(limiter: &DefaultDirectRateLimiter, seconds: u32) -> bool {
    let Some(seconds) = NonZeroU32::new(seconds) else {
        return true;
    };
    matches!(limiter.check_n(seconds), Ok(Ok(())))
}

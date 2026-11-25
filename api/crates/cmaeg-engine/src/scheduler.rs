//! Wake-up scheduler bounds, the work disposition and the drain report.
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Disposition {
    /// A durably terminal target, or an ambiguous effect awaiting reconciliation.
    Settled,
    /// Read-only recovery or a proved nonacceptance may be reconsidered later.
    Retry(Duration),
}

#[derive(Clone, Copy)]
pub struct ScheduleConfig {
    pub concurrency: usize,
    pub queue_capacity: usize,
    pub page_delay: Duration,
    pub sweep_delay: Duration,
    pub drain_timeout: Duration,
}
impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            queue_capacity: 512,
            page_delay: Duration::from_millis(250),
            sweep_delay: Duration::from_secs(5),
            drain_timeout: Duration::from_secs(660),
        }
    }
}
impl ScheduleConfig {
    pub fn valid(self) -> bool {
        (1..=256).contains(&self.concurrency)
            && (1..=4096).contains(&self.queue_capacity)
            && (Duration::from_millis(10)..=Duration::from_secs(60)).contains(&self.page_delay)
            && (Duration::from_millis(10)..=Duration::from_secs(300)).contains(&self.sweep_delay)
            && (Duration::from_millis(10)..=Duration::from_secs(660)).contains(&self.drain_timeout)
    }
}

#[derive(Default)]
pub struct ScheduleReport {
    pub started: u64,
    pub finished: u64,
    pub aborted: usize,
    pub recovery_failed: bool,
}
/// Stable admission and serialization identities for a durable wake-up.
pub trait WorkSelection: Clone + Send + 'static {
    fn key(&self) -> (String, String);
    fn serial_key(&self) -> (String, String);
}

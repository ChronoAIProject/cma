//! Reusable scenario runner. A real V1/V2 adapter must supply measured metrics;
//! the model adapter only tests this harness and coordinator sequencing.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metrics {
    pub format: SourceFormat,
    pub active_operations: u64,
    pub active_authority: u64,
    pub hot_progress_slots: usize,
    pub hot_replay_entries: usize,
    pub retained_groups: usize,
    pub active_bytes: u64,
    pub byte_ceiling: u64,
    pub page_geometry: Option<(u64, u64)>,
}

pub trait LifecycleDriver: CollectorPort + Sized {
    fn admit_completed_cycle(
        &self,
        cycle: u64,
    ) -> impl std::future::Future<Output = Result<usize>> + Send;
    fn reopen(&self) -> impl std::future::Future<Output = Result<Self>> + Send;
    fn metrics(&self) -> impl std::future::Future<Output = Result<Metrics>> + Send;
    fn original_ack(
        &self,
        operation: Operation,
    ) -> impl std::future::Future<Output = Result<Receipt>> + Send;
    fn reject_retired_replay(
        &self,
        intent: &Intent,
        higher_fence: bool,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

pub async fn until_complete<P: CollectorPort>(port: &P, max_steps: usize) -> Result<usize> {
    for i in 0..max_steps {
        if step(Admission { new_work: true }, port).await? == Step::InventoryComplete {
            return Ok(i + 1);
        }
    }
    Err(Refusal::UnknownOutcome)
}

pub async fn lifetime<D: LifecycleDriver>(driver: D, cycles: u64) -> Result<Metrics> {
    let baseline = driver.metrics().await?;
    let mut first = None;
    let mut driver = driver;
    let mut retained = baseline.retained_groups;
    for cycle in 1..=cycles {
        let groups = driver.admit_completed_cycle(cycle).await?;
        if !(1..=64).contains(&groups) {
            return Err(Refusal::InvalidEvidence);
        }
        retained = retained
            .checked_add(groups)
            .ok_or(Refusal::InvalidEvidence)?;
        // Capture the original admitted operation before the first source effect.
        for _ in 0..8 {
            if driver.load().await?.pending.is_some() {
                break;
            }
            step(Admission { new_work: true }, &driver).await?;
        }
        if driver.load().await?.pending.is_none() {
            return Err(Refusal::UnknownOutcome);
        }
        if cycle == 1 {
            first = Some(driver.load().await?.pending.unwrap().intent);
        }
        driver = driver.reopen().await?;
        until_complete(&driver, 32).await?;
        let metrics = driver.metrics().await?;
        if metrics.format != baseline.format
            || metrics.active_operations != 0
            || metrics.active_authority != 0
            || metrics.hot_progress_slots > 1
            || metrics.hot_replay_entries != baseline.hot_replay_entries
            || metrics.retained_groups != retained
            || metrics.byte_ceiling == 0
            || metrics.active_bytes > metrics.byte_ceiling
            || !matches!(
                (metrics.format, metrics.page_geometry),
                (SourceFormat::V1, None) | (SourceFormat::V2, Some((1.., 1..)))
            )
            || metrics
                .page_geometry
                .is_some_and(|(pages, ceiling)| pages > ceiling)
        {
            return Err(Refusal::InvalidEvidence);
        }
    }
    let first = first.ok_or(Refusal::InvalidEvidence)?;
    let original = driver.original_ack(first.operation).await?;
    if !original.matches(&first, Phase::Acknowledged) {
        return Err(Refusal::InvalidEvidence);
    }
    driver = driver.reopen().await?;
    if driver.original_ack(first.operation).await? != original {
        return Err(Refusal::InvalidEvidence);
    }
    driver.reject_retired_replay(&first, false).await?;
    driver.reject_retired_replay(&first, true).await?;
    driver.metrics().await
}

impl LifecycleDriver for model::Model {
    async fn admit_completed_cycle(&self, cycle: u64) -> Result<usize> {
        self.add_cycle(cycle);
        Ok(1)
    }
    async fn reopen(&self) -> Result<Self> {
        Ok(self.reopened())
    }
    async fn metrics(&self) -> Result<Metrics> {
        let s = self.state.lock().unwrap();
        let live = s.groups.len() as u64;
        Ok(Metrics {
            format: s.checkpoint.binding.format,
            active_operations: live * 6,
            active_authority: live * 3,
            hot_progress_slots: usize::from(s.staged.is_some()),
            hot_replay_entries: 0,
            retained_groups: s.cold.len(),
            active_bytes: (8 + live) * 4096,
            byte_ceiling: 256 * 4096,
            page_geometry: (s.checkpoint.binding.format == SourceFormat::V2)
                .then_some((8 + live, 256)),
        })
    }
    async fn original_ack(&self, operation: Operation) -> Result<Receipt> {
        self.state
            .lock()
            .unwrap()
            .cold
            .get(&operation)
            .and_then(|r| r.last())
            .cloned()
            .ok_or(Refusal::UnknownOutcome)
    }
    async fn reject_retired_replay(&self, intent: &Intent, _: bool) -> Result<()> {
        if self
            .state
            .lock()
            .unwrap()
            .cold
            .contains_key(&intent.operation)
        {
            Ok(())
        } else {
            Err(Refusal::InvalidEvidence)
        }
    }
}

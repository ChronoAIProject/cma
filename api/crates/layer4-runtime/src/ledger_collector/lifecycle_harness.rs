//! Reusable scenario runner. A real V1/V2 adapter must supply measured metrics;
//! the model adapter only tests this harness and coordinator sequencing.
use super::*;

#[path = "lifecycle_qualification.rs"]
mod qualification;
use qualification::Qualification;

/// Actual rows admitted by the fixture driver, independently counted again in
/// the completed source/archive outcome. These are not estimates from cycles.
pub struct CycleWork {
    pub groups: usize,
    pub operations: u64,
    pub authority: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metrics {
    pub format: SourceFormat,
    pub active_operations: u64,
    pub active_authority: u64,
    pub hot_progress_slots: usize,
    pub hot_replay_entries: usize,
    pub retained_groups: usize,
    pub reclaimed_operations: u64,
    pub reclaimed_authority: u64,
    /// Exact preserved live/unresolved source contents, excluding reclaimed rows.
    pub preserved_source_sha256: Digest,
    pub active_bytes: u64,
    pub byte_ceiling: u64,
    pub page_geometry: Option<(u64, u64)>,
}

pub trait LifecycleDriver: CollectorPort + Sized {
    fn admit_completed_cycle(
        &self,
        cycle: u64,
    ) -> impl std::future::Future<Output = Result<CycleWork>> + Send;
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
    if cycles == 0 || cycles > 16_384 {
        return Err(Refusal::InvalidEvidence);
    }
    let mut qualification = Qualification::new(driver.metrics().await?)?;
    let mut first = None;
    let mut driver = driver;
    for cycle in 1..=cycles {
        let work = driver.admit_completed_cycle(cycle).await?;
        let (reopened, intent) = complete_cycle(driver).await?;
        driver = reopened;
        first.get_or_insert(intent);
        qualification.record(&work, driver.metrics().await?)?;
    }
    qualification.require_legacy_bounds()?;
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

async fn complete_cycle<D: LifecycleDriver>(mut driver: D) -> Result<(D, Intent)> {
    let mut selected = None;
    // Bounded fixture campaign, including preserved groups on multiple pages.
    for _ in 0..1_024 {
        if let Some(pending) = driver.load().await?.pending {
            if selected.is_none() && pending.intent.decision == Decision::SettledDeleted {
                selected = Some(pending.intent);
                driver = driver.reopen().await?;
            }
        }
        if step(Admission { new_work: true }, &driver).await? == Step::InventoryComplete {
            return Ok((driver, selected.ok_or(Refusal::UnknownOutcome)?));
        }
    }
    Err(Refusal::UnknownOutcome)
}

impl LifecycleDriver for model::Model {
    async fn admit_completed_cycle(&self, cycle: u64) -> Result<CycleWork> {
        self.add_cycle(cycle);
        Ok(CycleWork {
            groups: 1,
            operations: 6,
            authority: 3,
        })
    }
    async fn reopen(&self) -> Result<Self> {
        Ok(self.reopened())
    }
    async fn metrics(&self) -> Result<Metrics> {
        use sha2::{Digest as _, Sha256};
        let s = self.state.lock().unwrap();
        let live = s.groups.len() as u64;
        let mut preserved = Sha256::new();
        for (group, decision) in &s.groups {
            if *decision != Decision::SettledDeleted {
                preserved.update(group);
                preserved.update([if *decision == Decision::RetainLive {
                    1
                } else {
                    2
                }]);
            }
        }
        Ok(Metrics {
            format: s.checkpoint.binding.format,
            active_operations: live * 6,
            active_authority: live * 3,
            hot_progress_slots: usize::from(s.staged.is_some()),
            hot_replay_entries: 0,
            retained_groups: s.cold.len(),
            reclaimed_operations: s.retired_operations,
            reclaimed_authority: s.retired_authority,
            preserved_source_sha256: preserved.finalize().into(),
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

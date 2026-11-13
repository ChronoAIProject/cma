//! Background online provenance capture, outside startup readiness and user work.
use layer2_core::sandbox::store::{
    AgentProvenanceBackfillStore, PageRequest, ProvenanceBackfillOutcome,
    ProvenanceBackfillProgress,
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

const PAGE_SIZE: u16 = 25;
const ITEM_DELAY: Duration = Duration::from_millis(250);
const SWEEP_DELAY: Duration = Duration::from_secs(300);
const IO_DEADLINE: Duration = Duration::from_secs(15);

pub(crate) struct AgentProvenanceBackfill {
    store: Arc<dyn AgentProvenanceBackfillStore>,
    budget: Arc<crate::background::BackgroundBudget>,
}
impl AgentProvenanceBackfill {
    pub(crate) fn new(store: Arc<dyn AgentProvenanceBackfillStore>) -> Self {
        Self {
            store,
            budget: crate::background::shared(),
        }
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = crate::shutdown::signalled(shutdown.clone()) => (),
                _ = self.run(&mut shutdown) => (),
            }
        })
    }

    async fn run(&self, shutdown: &mut watch::Receiver<bool>) {
        // Leave startup and initial user reconnection free of migration traffic.
        tokio::time::sleep(Duration::from_secs(30)).await;
        let scan = crate::background::BackgroundWork::new(
            self.budget.clone(),
            "provenance_scan",
            IO_DEADLINE,
            Duration::ZERO,
        );
        let item = crate::background::BackgroundWork::new(
            self.budget.clone(),
            "provenance_item",
            IO_DEADLINE,
            ITEM_DELAY,
        );
        let worker = uuid::Uuid::new_v4().to_string();
        let mut activated = false;
        let mut page = PageRequest::new(PAGE_SIZE, None).expect("fixed bounded page");
        loop {
            if *shutdown.borrow() {
                return;
            }
            match item
                .run(|| self.store.durable_provenance_backfill_step(&worker))
                .await
            {
                Ok(Some(Some(progress))) => {
                    activated = true;
                    page.cursor = None;
                    match progress {
                        ProvenanceBackfillProgress::Advanced => (),
                        ProvenanceBackfillProgress::Busy => {
                            tokio::time::sleep(Duration::from_secs(5)).await
                        }
                        ProvenanceBackfillProgress::SweepFinished => {
                            tokio::time::sleep(SWEEP_DELAY).await
                        }
                    }
                    continue;
                }
                Ok(Some(None)) if !activated => (),
                Ok(Some(None)) => {
                    tracing::warn!(
                        "Activated provenance progress disappeared; maintenance refused"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                Ok(None) => continue,
                Err(_) => {
                    tracing::warn!("Durable provenance progress unavailable; retrying later");
                    continue;
                }
            }
            let batch = match scan
                .run(|| self.store.scan_agent_provenance_backfill(&page))
                .await
            {
                Ok(Some(batch)) => batch,
                Ok(None) => continue,
                Err(_) => {
                    tracing::warn!("Agent provenance backfill scan unavailable; retrying later");
                    continue;
                }
            };
            let mut recorded = 0_u16;
            let mut refused = 0_u16;
            for id in &batch.items {
                loop {
                    match item.run(|| self.store.backfill_agent_provenance(id)).await {
                        Ok(Some(ProvenanceBackfillOutcome::Recorded)) => recorded += 1,
                        Ok(Some(ProvenanceBackfillOutcome::Retained)) => (),
                        Ok(None) => continue,
                        Err(_) => refused += 1,
                    }
                    break;
                }
            }
            if recorded > 0 || refused > 0 {
                tracing::info!(
                    recorded,
                    refused,
                    "Agent provenance backfill page processed"
                );
            }
            // A refused/corrupt row cannot prevent progress through later IDs.
            // Revisit it in the next sweep; restarts safely rescan immutable rows.
            if let Some(next) = batch.next {
                if batch.items.last().is_none_or(|id| id.as_str() != next)
                    || page
                        .cursor
                        .as_ref()
                        .is_some_and(|previous| previous >= &next)
                {
                    tracing::warn!("Agent provenance backfill scan did not advance");
                    tokio::time::sleep(SWEEP_DELAY).await;
                } else {
                    page.cursor = Some(next);
                }
            } else {
                page.cursor = None;
                tokio::time::sleep(SWEEP_DELAY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;

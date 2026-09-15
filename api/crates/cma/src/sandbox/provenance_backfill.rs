//! Background online provenance capture, outside startup readiness and user work.
use layer2_core::sandbox::store::{
    AgentProvenanceBackfillStore, PageRequest, ProvenanceBackfillOutcome,
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

const PAGE_SIZE: u16 = 25;
const ITEM_DELAY: Duration = Duration::from_millis(250);
const SWEEP_DELAY: Duration = Duration::from_secs(300);
const IO_DEADLINE: Duration = Duration::from_secs(15);

pub(crate) struct AgentProvenanceBackfill {
    store: Arc<dyn AgentProvenanceBackfillStore>,
}
impl AgentProvenanceBackfill {
    pub(crate) fn new(store: Arc<dyn AgentProvenanceBackfillStore>) -> Self {
        Self { store }
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
        let mut page = PageRequest::new(PAGE_SIZE, None).expect("fixed bounded page");
        loop {
            if *shutdown.borrow() {
                return;
            }
            let batch = match tokio::time::timeout(
                IO_DEADLINE,
                self.store.scan_agent_provenance_backfill(&page),
            )
            .await
            {
                Ok(Ok(batch)) => batch,
                _ => {
                    tracing::warn!("Agent provenance backfill scan unavailable; retrying later");
                    tokio::time::sleep(SWEEP_DELAY).await;
                    continue;
                }
            };
            let mut recorded = 0_u16;
            let mut refused = 0_u16;
            for id in &batch.items {
                match tokio::time::timeout(IO_DEADLINE, self.store.backfill_agent_provenance(id))
                    .await
                {
                    Ok(Ok(ProvenanceBackfillOutcome::Recorded)) => recorded += 1,
                    Ok(Ok(ProvenanceBackfillOutcome::Retained)) => (),
                    _ => refused += 1,
                }
                tokio::time::sleep(ITEM_DELAY).await;
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

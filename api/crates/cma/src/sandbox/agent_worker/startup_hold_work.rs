//! Start-up hold integration at the discovery and claim boundaries.
use super::startup_holds::{Admission, Binding, ProbePermit};
use super::*;
use layer2_core::sandbox::store::{AgentWorkDiscovery, StartupWorkFilter, MAX_PAGE_SIZE};
use std::collections::BTreeSet;
use tokio::time::Instant;

const BINDING_CHECK_TIMEOUT: Duration = Duration::from_millis(150);

impl AgentWorker {
    pub(in crate::sandbox) fn next_startup_probe(&self) -> Option<Instant> {
        self.startup_holds.as_ref()?.next_probe()
    }

    pub(in crate::sandbox) fn observe_runtime_startup<T>(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
        started: Instant,
        result: &Result<T, layer3_adapter::AdapterError>,
    ) {
        if let Some(holds) = &self.startup_holds {
            holds.observe(sandbox, started, result, Instant::now());
        }
    }

    pub(super) async fn due_with_startup_holds(
        &self,
        now: &str,
        limit: u16,
        cleanup: &BTreeSet<AgentId>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let store = &self.dependencies.store;
        let cooldowns = self.acquisition_backoff.cooling(Instant::now());
        let mut filter = AgentWorkDiscovery {
            receiver_holds: cleanup,
            startup: StartupWorkFilter::All,
            acquisition_cooldowns: (!cooldowns.is_empty()).then_some(&cooldowns),
        };
        let Some(holds) = self.startup_holds.as_ref().filter(|holds| holds.pending()) else {
            return store.due_agent_work_filtered(now, limit, filter).await;
        };
        // These bounded, read-only checks also reopen discovery on a changed
        // binding when every sibling is currently excluded from the normal page.
        futures_util::future::join_all(holds.refresh_candidates(Instant::now()).into_iter().map(
            |check| async move {
                let current =
                    tokio::time::timeout(BINDING_CHECK_TIMEOUT, store.get_sandbox(&check.id)).await;
                let binding = match current {
                    Ok(Ok(sandbox)) => sandbox.as_ref().and_then(Binding::current),
                    Ok(Err(_)) | Err(_) => {
                        tracing::debug!(
                            "Agent startup hold binding proof unavailable during discovery"
                        );
                        None
                    }
                };
                holds.refreshed(check, binding, Instant::now());
            },
        ))
        .await;
        let discovery = holds.discovery(Instant::now(), limit);
        let mut ids = Vec::new();
        if let Some(sandbox) = &discovery.probe {
            filter.startup = StartupWorkFilter::Probe(sandbox);
            match store.due_agent_work_filtered(now, 1, filter).await {
                Ok(probe_ids) => ids = probe_ids,
                Err(_) => {
                    tracing::warn!(sandbox_id = %sandbox, "Agent startup probe discovery unavailable")
                }
            }
        }
        if ids.len() < usize::from(limit) {
            let remaining = limit - ids.len() as u16;
            filter.startup = if discovery.excluded.is_empty() {
                StartupWorkFilter::All
            } else {
                StartupWorkFilter::Exclude(&discovery.excluded)
            };
            let probe = ids.first().cloned();
            let ordinary = store
                .due_agent_work_filtered(now, remaining, filter)
                .await?;
            ids.extend(ordinary.into_iter().filter(|id| Some(id) != probe.as_ref()));
        }
        Ok(ids)
    }

    pub(super) async fn startup_admission<'a>(
        &'a self,
        id: &AgentId,
        existing: Option<&ProbePermit<'_>>,
        may_start_probe: bool,
    ) -> Admission<'a> {
        let Some(holds) = self.startup_holds.as_ref().filter(|holds| holds.pending()) else {
            return Admission::Proceed;
        };
        // These reads only establish the advisory hold. Share one deadline
        // and leave authority and claim validation to the ordinary path.
        let deadline = Instant::now() + BINDING_CHECK_TIMEOUT;
        let store = &self.dependencies.store;
        let agent = match tokio::time::timeout_at(deadline, store.get_agent(id)).await {
            Ok(Ok(Some(agent))) if StartupWorkFilter::can_hold(&agent) => agent,
            Ok(Ok(_)) => return Admission::Proceed,
            Ok(Err(_)) | Err(_) => {
                tracing::debug!("Agent startup hold could not validate the current work step");
                return Admission::Proceed;
            }
        };
        if !holds.holds(&agent.sandbox_id) {
            return Admission::Proceed;
        }
        let binding =
            match tokio::time::timeout_at(deadline, store.get_sandbox(&agent.sandbox_id)).await {
                Ok(Ok(sandbox)) => sandbox.as_ref().and_then(Binding::current),
                Ok(Err(_)) | Err(_) => {
                    tracing::debug!(
                        "Agent startup hold could not validate the current runtime binding"
                    );
                    return Admission::Proceed;
                }
            };
        let decision = holds.admit(
            &agent.sandbox_id,
            id,
            binding,
            existing,
            Instant::now(),
            may_start_probe,
        );
        match &decision {
            Admission::Hold => {
                metrics::counter!("cma_agent_runtime_startup_hold_suppressed_total").increment(1)
            }
            Admission::Probe(_) => {
                metrics::counter!("cma_agent_runtime_startup_hold_probes_total").increment(1)
            }
            Admission::Proceed => {}
        }
        decision
    }
}

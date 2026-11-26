//! Observe admitted turns without competing with their existing output recorder.

use std::{collections::HashMap, sync::Arc, time::Duration};

use layer2_core::{
    agent_ownership::OwnershipCoordinator,
    sandbox::store::{SandboxStoreError, StaleTurnSettlement},
};
use layer3_adapter::{
    stale_turn::{StaleTurnHold, StaleTurnProducer, MAX_SEAL_BYTES},
    RuntimeHandle, SandboxAdapter,
};
use mongodb::bson::DateTime;
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

use super::LiveAgentRegistry;
use crate::{
    background::{self, BackgroundWork},
    store::{
        stale_turns::{Candidate, Observation, ProofBudget, StaleTurnStore},
        MongoSandboxStore,
    },
    worker_supervision::{self, Activity},
};

const NAME: &str = "agent_stale_turn_reconciliation";
const INTERVAL: Duration = Duration::from_secs(30);
const SWEEP_DEADLINE: Duration = Duration::from_secs(10);
const INVESTIGATION_DEADLINE: Duration = Duration::from_secs(5);
const MAX_INVESTIGATIONS: usize = 4;
const HOLD_CAPACITY: usize = 512;
const HOLD_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(test)]
mod partial_tests;
#[cfg(test)]
pub(crate) use partial_tests::assert_partial_does_not_request_stop;

pub(super) struct Worker {
    store: StaleTurnStore,
    ownership: Arc<OwnershipCoordinator>,
    registry: Arc<LiveAgentRegistry>,
    terminal_adapter: Option<Arc<dyn SandboxAdapter>>,
    work: BackgroundWork,
    reported: HashMap<Candidate, (&'static str, Instant)>,
}

impl Worker {
    pub(super) fn new(
        store: Arc<MongoSandboxStore>,
        ownership: Arc<OwnershipCoordinator>,
        registry: Arc<LiveAgentRegistry>,
        terminal_adapter: Option<Arc<dyn SandboxAdapter>>,
    ) -> Self {
        Self {
            store: StaleTurnStore::new(store),
            ownership,
            registry,
            terminal_adapter,
            work: BackgroundWork::new(background::shared(), NAME, SWEEP_DEADLINE, INTERVAL),
            reported: HashMap::new(),
        }
    }

    pub(super) fn spawn(
        self,
        shutdown: watch::Receiver<bool>,
        activity: Activity,
    ) -> JoinHandle<()> {
        worker_supervision::spawn(
            self,
            activity,
            shutdown,
            |worker, activity, mut shutdown| {
                Box::pin(async move {
                    let mut ticker = tokio::time::interval(INTERVAL);
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(INTERVAL + SWEEP_DEADLINE, "stale_turn_scan");
                        tokio::select! {
                            _ = shutdown.changed() => break,
                            _ = ticker.tick() => {
                                activity.scheduled();
                                let _scan = activity.reconciliation();
                                let work = worker.work.clone();
                                if work.run(|| worker.sweep()).await.is_err() {
                                    outcome("sweep_unavailable");
                                }
                            }
                        }
                    }
                })
            },
            |_| Box::pin(async {}),
        )
    }

    async fn sweep(&mut self) -> Result<(), SandboxStoreError> {
        if !self.ownership.is_ready() {
            outcome("ownership_unavailable");
            return Ok(());
        }
        let Some(lease) = self.store.claim().await.map_err(store_error)? else {
            outcome("scan_busy");
            return Ok(());
        };
        let page = self.store.page(&lease).await.map_err(store_error)?;
        let stop_investigating = Instant::now() + INVESTIGATION_DEADLINE;
        self.reported
            .retain(|_, (_, until)| *until > Instant::now());
        let mut last_examined = None;
        let mut examined = 0;
        let mut investigations = 0;
        let mut oldest = None;
        for row in &page.rows {
            if !self.ownership.is_ready()
                || investigations == MAX_INVESTIGATIONS
                || Instant::now() >= stop_investigating
            {
                break;
            }
            let key = row.get_str("_id").map_err(|_| {
                SandboxStoreError::Corrupt("stale-turn scan cursor is invalid".into())
            })?;
            last_examined = Some(key);
            examined += 1;
            let Some(candidate) = Candidate::from_row(row, lease.now) else {
                continue;
            };
            if let Some(reason) = Candidate::early_hold(row) {
                self.report_hold(candidate, reason);
                continue;
            }
            investigations += 1;
            let mut budget = ProofBudget::default();
            let observation = tokio::time::timeout_at(stop_investigating, async {
                if self.terminal_adapter.is_some() {
                    self.store
                        .observe_for_terminal(&candidate, lease.now, &mut budget)
                        .await
                } else {
                    self.store.observe(&candidate, lease.now, &mut budget).await
                }
            })
            .await;
            let reason = match observation {
                Ok(Ok(Observation::Progressing)) => {
                    self.reported.remove(&candidate);
                    outcome("progressing");
                    continue;
                }
                Ok(Ok(Observation::Hold(reason))) => reason,
                Ok(Ok(Observation::Quiet {
                    target,
                    age,
                    evidence,
                })) => {
                    oldest = Some(oldest.map_or(age, |previous: u64| previous.max(age)));
                    let terminal = tokio::time::timeout_at(
                        stop_investigating,
                        self.try_terminal(
                            &candidate,
                            lease.now,
                            target,
                            evidence,
                            &mut budget,
                            |runtime| self.terminal_adapter.as_ref()?.stale_turn_producer(runtime),
                        ),
                    )
                    .await;
                    match terminal {
                        Ok(Ok(())) => {
                            self.reported.remove(&candidate);
                            outcome("terminal_recorded");
                            continue;
                        }
                        Ok(Err(reason)) => reason,
                        Err(_) => "terminal_investigation_timeout",
                    }
                }
                Ok(Err(error)) => error.outcome(),
                Err(_) => "observation_timeout",
            };
            self.report_hold(candidate, reason);
        }
        metrics::histogram!("cma_stale_turn_scan_rows").record(examined as f64);
        if let Some(age) = oldest {
            metrics::histogram!("cma_stale_turn_oldest_eligible_age_seconds").record(age as f64);
        }
        let eof = page.eof && examined == page.rows.len();
        if self
            .store
            .advance(&lease, last_examined, eof)
            .await
            .map_err(store_error)?
        {
            outcome("complete");
        } else {
            outcome("scan_lease_lost");
        }
        Ok(())
    }

    async fn try_terminal<'producer>(
        &self,
        candidate: &Candidate,
        now: DateTime,
        target: super::LiveAgentTarget,
        evidence: Box<crate::store::stale_turns::QuietTurn>,
        budget: &mut ProofBudget,
        producer_for: impl FnOnce(&RuntimeHandle) -> Option<&'producer dyn StaleTurnProducer>,
    ) -> Result<(), &'static str> {
        let authority = self
            .ownership
            .local(&target.agent_id)
            .await
            .ok_or("owner_elsewhere")?;
        authority
            .run(async {
                if !self
                    .registry
                    .owned_target_is_recording(&target, &authority.token)
                {
                    return Err("recorder_unavailable");
                }
                let expected = StaleTurnSettlement::receipt_target(
                    &evidence.snapshot,
                    &evidence.parent,
                    &evidence.command,
                    &authority.token,
                )
                .map_err(|_| "terminal_evidence_unavailable")?;
                let producer =
                    producer_for(&expected.runtime).ok_or("producer_seal_unavailable")?;
                // Reserve the response allowance even if decoding fails or the
                // probe is cancelled after reading part of the body.
                let allowance = budget.remaining().min(MAX_SEAL_BYTES as i64);
                if allowance == 0 {
                    return Err("proof_byte_budget");
                }
                budget.charge(allowance).map_err(|error| error.outcome())?;
                let producer_evidence = match producer
                    .observe_stale_turn_seal(&expected, allowance as usize)
                    .await
                {
                    Ok(evidence) => evidence,
                    Err(StaleTurnHold::Unsupported) => return Err("producer_seal_unavailable"),
                    Err(StaleTurnHold::CanonicalResult) => return Err("canonical_result"),
                    Err(StaleTurnHold::Unconfirmed) => {
                        // A partial candidate may only read a stop already
                        // admitted while the assembly was empty.
                        if evidence
                            .snapshot
                            .checkpoint
                            .as_ref()
                            .is_some_and(|checkpoint| !checkpoint.assembly.is_empty())
                        {
                            return Err("output_uncommitted");
                        }
                        let fresh = self
                            .store
                            .observe(candidate, now, budget)
                            .await
                            .map_err(|error| error.outcome())?;
                        let Observation::Quiet {
                            target: current,
                            evidence,
                            ..
                        } = fresh
                        else {
                            return Err("identity_changed");
                        };
                        let confirmed = StaleTurnSettlement::target(
                            &evidence.snapshot,
                            &evidence.parent,
                            &evidence.command,
                            &authority.token,
                        )
                        .map_err(|_| "terminal_evidence_unavailable")?;
                        if current != target || confirmed != expected {
                            return Err("identity_changed");
                        }
                        if !self
                            .registry
                            .owned_target_is_recording(&target, &authority.token)
                        {
                            return Err("recorder_unavailable");
                        }
                        return match producer.request_stale_turn_stop(&confirmed).await {
                            Ok(()) => Err("producer_stop_pending"),
                            Err(StaleTurnHold::CanonicalResult) => Err("canonical_result"),
                            Err(StaleTurnHold::Unsupported | StaleTurnHold::Unconfirmed) => {
                                Err("producer_seal_unconfirmed")
                            }
                        };
                    }
                };
                let seal = producer_evidence.seal;
                if !seal.matches(&expected) {
                    return Err("producer_seal_unconfirmed");
                }
                let seal_bytes = serde_json::to_vec(&seal)
                    .map_err(|_| "producer_seal_unconfirmed")?
                    .len();
                if producer_evidence.bytes_read.max(seal_bytes) > allowance as usize {
                    return Err("proof_byte_budget");
                }
                // Read a new majority snapshot after the probe. Only the attached
                // recorder can drain retained frames and advance this checkpoint.
                let fresh = self
                    .store
                    .observe_for_terminal(candidate, now, budget)
                    .await
                    .map_err(|error| error.outcome())?;
                let evidence = match fresh {
                    Observation::Quiet {
                        target: current,
                        evidence,
                        ..
                    } if current == target => evidence,
                    Observation::Hold(reason) => return Err(reason),
                    Observation::Progressing => return Err("progressing"),
                    Observation::Quiet { .. } => return Err("identity_changed"),
                };
                if !self
                    .registry
                    .owned_target_is_recording(&target, &authority.token)
                {
                    return Err("recorder_unavailable");
                }
                let settlement = StaleTurnSettlement::prepare(
                    evidence.snapshot,
                    evidence.parent,
                    evidence.command,
                    authority.token.clone(),
                    seal,
                    layer2_core::now_rfc3339(),
                )
                .map_err(|_| "terminal_evidence_unavailable")?;
                Box::pin(self.store.settle(&settlement, budget))
                    .await
                    .map_err(|error| match error {
                        _ if budget.remaining() == 0 => "proof_byte_budget",
                        SandboxStoreError::Conflict => "terminal_conflict",
                        _ => "terminal_unconfirmed",
                    })
            })
            .await
            .map_err(|_| "owner_changed")?
    }

    fn report_hold(&mut self, candidate: Candidate, reason: &'static str) {
        if self
            .reported
            .get(&candidate)
            .is_some_and(|(previous, _)| *previous == reason)
        {
            return;
        }
        if self.reported.len() < HOLD_CAPACITY || self.reported.contains_key(&candidate) {
            self.reported
                .insert(candidate, (reason, Instant::now() + HOLD_INTERVAL));
        }
        metrics::counter!("cma_stale_turn_hold_total", "reason" => reason).increment(1);
    }
}

fn outcome(outcome: &'static str) {
    metrics::counter!("cma_stale_turn_reconciliation_total", "outcome" => outcome).increment(1);
}

fn store_error(error: crate::store::stale_turns::ScanError) -> SandboxStoreError {
    outcome(error.outcome());
    SandboxStoreError::Unavailable(error.to_string())
}

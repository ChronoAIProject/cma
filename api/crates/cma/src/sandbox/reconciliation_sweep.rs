//! Checkpoints only move after a bounded page/item has returned successfully.
use super::{
    authority::Authority,
    corrupt, provider_error,
    telemetry::{Phase, Telemetry},
    SandboxReconciler,
};
use layer2_core::{
    rfc3339_at,
    sandbox::store::{PageRequest, SandboxInactivityPauseRequest, SandboxStoreError},
    SandboxId,
};
use layer3_adapter::{agent_sandbox::NativePoolClaim, HomeVolumeAttachment, RuntimeObservation};
use std::{
    collections::VecDeque,
    time::{Duration, SystemTime},
};

// Leave five seconds of margin under BackgroundWork's 30-second hard bound.
// Every external call is inside one of these bounded steps, including grouped
// fresh ownership checks + cleanup. No pending I/O survives a yielded attempt.
const SLICE: Duration = Duration::from_secs(20);
const OPERATION: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Progress {
    sweep: Sweep,
    telemetry: Telemetry,
}

struct Sweep {
    phase: Phase,
    inventory: Vec<RuntimeObservation>,
    authority: Authority,
    binding_cursor: Option<SandboxId>,
    recovery_candidate: Option<SandboxId>,
    runtime_index: usize,
    claim_inventory: Option<Box<dyn layer3_adapter::InventoryCursor<NativePoolClaim>>>,
    home_inventory: Option<Box<dyn layer3_adapter::InventoryCursor<HomeVolumeAttachment>>>,
    claims: VecDeque<NativePoolClaim>,
    homes: VecDeque<HomeVolumeAttachment>,
    candidates: VecDeque<SandboxId>,
    candidate_page: PageRequest,
    candidates_finished: bool,
    pause_after_seconds: u64,
}

impl Default for Sweep {
    fn default() -> Self {
        Self {
            phase: Phase::Inventory,
            inventory: Vec::new(),
            authority: Authority::default(),
            binding_cursor: None,
            recovery_candidate: None,
            runtime_index: 0,
            claim_inventory: None,
            home_inventory: None,
            claims: VecDeque::new(),
            homes: VecDeque::new(),
            candidates: VecDeque::new(),
            candidate_page: PageRequest::default(),
            candidates_finished: false,
            pause_after_seconds: 0,
        }
    }
}

impl SandboxReconciler {
    pub(in crate::sandbox) async fn run_due(
        &self,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let mut progress = self.progress.lock().await;
        let Progress { sweep, telemetry } = progress.get_or_insert_with(Progress::default);
        let mut attempt = telemetry.attempt();
        let started = tokio::time::Instant::now();
        let complete = loop {
            attempt.start(sweep.phase);
            match tokio::time::timeout(OPERATION, sweep.step(self, now)).await {
                Ok(Ok(items)) => attempt.record(items),
                Ok(Err(error)) => {
                    attempt.finish("unavailable");
                    return Err(error);
                }
                Err(_) => {
                    attempt.finish("deadline");
                    return Err(SandboxStoreError::Unavailable(
                        "sandbox maintenance operation deadline exceeded".into(),
                    ));
                }
            }
            if sweep.phase == Phase::Complete {
                break true;
            }
            if started.elapsed() >= SLICE {
                break false;
            }
            tokio::task::yield_now().await;
        };
        attempt.finish(if complete { "complete" } else { "yielded" });
        drop(attempt);
        if complete {
            *progress = None;
        }
        Ok(complete)
    }
}

impl Sweep {
    async fn step(
        &mut self,
        reconciler: &SandboxReconciler,
        now: SystemTime,
    ) -> Result<u64, SandboxStoreError> {
        match self.phase {
            Phase::Inventory => {
                self.inventory = reconciler
                    .adapter
                    .runtime_inventory()
                    .await
                    .map_err(provider_error)?;
                self.phase = Phase::Authority;
            }
            Phase::Authority | Phase::RecoveryAuthority => {
                let page = reconciler
                    .store
                    .sandbox_runtime_authority(&self.authority.request)
                    .await?;
                let items = page.items.len() as u64;
                if self.authority.accept(page)? {
                    self.phase = if self.phase == Phase::Authority {
                        Phase::Bindings
                    } else {
                        self.recovery_candidate =
                            reconciler.failed_runtime_recovery_candidate(&self.authority.items);
                        Phase::Recovery
                    };
                }
                return Ok(items);
            }
            Phase::Bindings => {
                use std::ops::Bound::{Excluded, Unbounded};
                let next = self
                    .authority
                    .items
                    .range((
                        self.binding_cursor.as_ref().map_or(Unbounded, Excluded),
                        Unbounded,
                    ))
                    .next()
                    .map(|(id, _)| id.clone());
                if let Some(id) = next {
                    // A retained page is discovery only. Reread before adoption
                    // or recovery so a previous attempt's write is replay-safe.
                    if let Some(sandbox) = reconciler.store.get_sandbox(&id).await? {
                        sandbox
                            .validate()
                            .map_err(|error| corrupt(&error.to_string()))?;
                        reconciler
                            .reconcile_binding(&sandbox, &self.inventory, now)
                            .await?;
                    }
                    self.binding_cursor = Some(id);
                    return Ok(1);
                }
                self.authority = Authority::default();
                self.phase = Phase::Runtimes;
            }
            Phase::Runtimes => {
                if let Some(observed) = self.inventory.get(self.runtime_index) {
                    reconciler.reconcile_runtime(observed).await?;
                    self.runtime_index += 1;
                    return Ok(1);
                }
                self.inventory.clear();
                self.phase = Phase::ClaimInventory;
            }
            Phase::ClaimInventory => {
                if let Some(pools) = reconciler.adapter.native_pools() {
                    if self.claim_inventory.is_none() {
                        self.claim_inventory = pools.claim_inventory_cursor();
                    }
                    let items = match &mut self.claim_inventory {
                        Some(cursor) => cursor.advance().await.map_err(provider_error)?,
                        None => Some(pools.inventory_claims().await.map_err(provider_error)?),
                    };
                    let Some(items) = items else { return Ok(0) };
                    self.claims = items.into();
                    self.claim_inventory = None;
                }
                self.phase = Phase::Claims;
            }
            Phase::Claims => {
                if let Some(claim) = self.claims.front() {
                    reconciler.reconcile_native_claim(claim).await?;
                    self.claims.pop_front();
                    return Ok(1);
                }
                self.phase = Phase::HomeInventory;
            }
            Phase::HomeInventory => {
                if let Some(homes) = reconciler.adapter.home_volumes() {
                    if self.home_inventory.is_none() {
                        self.home_inventory = homes.inventory_cursor();
                    }
                    let items = match &mut self.home_inventory {
                        Some(cursor) => cursor.advance().await.map_err(provider_error)?,
                        None => Some(homes.inventory().await.map_err(provider_error)?),
                    };
                    let Some(items) = items else { return Ok(0) };
                    self.homes = items.into();
                    self.home_inventory = None;
                }
                self.phase = Phase::Homes;
            }
            Phase::Homes => {
                if let Some(home) = self.homes.front() {
                    reconciler.reconcile_home(home).await?;
                    self.homes.pop_front();
                    return Ok(1);
                }
                self.phase = if reconciler.failed_runtime_recovery.is_some() {
                    Phase::RecoveryAuthority
                } else {
                    Phase::Policy
                };
            }
            Phase::Recovery => {
                if let Some(id) = &self.recovery_candidate {
                    reconciler.recover_failed_unpublished_runtime(id).await;
                }
                self.authority = Authority::default();
                self.phase = Phase::Policy;
                return Ok(u64::from(self.recovery_candidate.is_some()));
            }
            Phase::Policy => {
                self.pause_after_seconds = reconciler
                    .policy
                    .current_sandbox_policy()
                    .await
                    .map_err(|_| {
                        SandboxStoreError::Unavailable(
                            "sandbox maintenance policy is unavailable".into(),
                        )
                    })?
                    .inactivity_pause_after_seconds;
                self.phase = Phase::Candidates;
            }
            Phase::Candidates => {
                if self.candidates_finished {
                    self.phase = Phase::Complete;
                } else {
                    let page = reconciler
                        .store
                        .sandbox_maintenance_candidates(&self.candidate_page)
                        .await?;
                    let mut previous = self.candidate_page.cursor.as_deref();
                    for id in &page.items {
                        if previous.is_some_and(|previous| previous >= id.as_str()) {
                            return Err(corrupt(
                                "maintenance candidate scan is not strictly ordered",
                            ));
                        }
                        previous = Some(id.as_str());
                    }
                    if page.items.len() > usize::from(self.candidate_page.limit)
                        || page.next.as_ref().is_some_and(|next| {
                            page.items.is_empty() || previous != Some(next.as_str())
                        })
                    {
                        return Err(corrupt("maintenance candidate scan did not advance"));
                    }
                    self.candidates_finished = page.next.is_none();
                    self.candidate_page.cursor = page.next;
                    self.candidates = page.items.into();
                    if !self.candidates.is_empty() {
                        self.phase = Phase::Profiles;
                    }
                }
            }
            Phase::Profiles => {
                let id = self
                    .candidates
                    .front()
                    .expect("candidate is retained until admission finishes");
                if let Some(agents) = &reconciler.profile_agents {
                    if agents.reconcile_profile_agents(id).await.is_err() {
                        tracing::warn!(parent: None, phase = "profiles", reason = "unavailable", "automatic profile agent reconciliation failed");
                    }
                }
                self.phase = Phase::RuntimeUpdates;
                return Ok(1);
            }
            Phase::RuntimeUpdates => {
                let id = self
                    .candidates
                    .front()
                    .expect("candidate is retained until admission finishes");
                if reconciler.reconcile_runtime_update(id, now).await? {
                    self.next_candidate();
                } else {
                    self.phase = Phase::IdlePause;
                }
                return Ok(1);
            }
            Phase::IdlePause => {
                let sandbox_id = self
                    .candidates
                    .front()
                    .expect("candidate is retained until admission finishes")
                    .clone();
                reconciler
                    .store
                    .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                        sandbox_id,
                        requested_at: rfc3339_at(now),
                        pause_after_seconds: self.pause_after_seconds,
                    })
                    .await?;
                self.next_candidate();
                return Ok(1);
            }
            Phase::Complete => {}
        }
        Ok(0)
    }

    fn next_candidate(&mut self) {
        self.candidates.pop_front();
        self.phase = if self.candidates.is_empty() {
            Phase::Candidates
        } else {
            Phase::Profiles
        };
    }
}

#[cfg(test)]
#[path = "reconciliation_sweep_tests.rs"]
mod tests;

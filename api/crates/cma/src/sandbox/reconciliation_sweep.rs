//! Failed bindings advance under a protective hold; other checkpoints require success.
use super::{
    authority::Authority,
    corrupt, provider_error,
    telemetry::{Phase, Telemetry},
    SandboxReconciler,
};
use layer2_core::{
    rfc3339_at,
    sandbox::store::{
        InactivityPauseCause, PageRequest, SandboxInactivityPauseRequest, SandboxStoreError,
    },
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
const BINDING_OPPORTUNITY: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(super) struct Progress {
    sweep: Sweep,
    binding_refresh: Option<Sweep>,
    refresh_required: bool,
    telemetry: Telemetry,
}

struct Sweep {
    phase: Phase,
    inventory: Vec<RuntimeObservation>,
    authority: Authority,
    binding_cursor: Option<SandboxId>,
    pending_binding: Option<SandboxId>,
    runtime_index: usize,
    claim_inventory: Option<Box<dyn layer3_adapter::InventoryCursor<NativePoolClaim>>>,
    home_inventory: Option<Box<dyn layer3_adapter::InventoryCursor<HomeVolumeAttachment>>>,
    claims: VecDeque<NativePoolClaim>,
    homes: VecDeque<HomeVolumeAttachment>,
    candidates: VecDeque<SandboxId>,
    candidate_page: PageRequest,
    candidates_finished: bool,
    platform_stop: bool,
    explicit_updates_only: bool,
}

impl Default for Sweep {
    fn default() -> Self {
        Self {
            phase: Phase::Inventory,
            inventory: Vec::new(),
            authority: Authority::default(),
            binding_cursor: None,
            pending_binding: None,
            runtime_index: 0,
            claim_inventory: None,
            home_inventory: None,
            claims: VecDeque::new(),
            homes: VecDeque::new(),
            candidates: VecDeque::new(),
            candidate_page: PageRequest::default(),
            candidates_finished: false,
            platform_stop: false,
            explicit_updates_only: false,
        }
    }
}

#[path = "reconciliation_schedule.rs"]
mod schedule;

impl Sweep {
    fn next_binding(&self) -> Option<SandboxId> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.authority
            .items
            .range((
                self.binding_cursor.as_ref().map_or(Unbounded, Excluded),
                Unbounded,
            ))
            .next()
            .map(|(id, _)| id.clone())
    }

    fn reserved_binding_due(&self, reconciler: &SandboxReconciler) -> bool {
        self.phase == Phase::Bindings
            && self.next_binding().is_some_and(|id| {
                reconciler
                    .binding_holds
                    .reserved_retry_due(&id, reconciler.runtime_loss_bounded_proof_enabled)
            })
    }

    fn is_binding_phase(&self) -> bool {
        matches!(
            self.phase,
            Phase::Inventory | Phase::Authority | Phase::Bindings
        )
    }

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
            Phase::Authority => {
                let page = reconciler
                    .store
                    .sandbox_runtime_authority(&self.authority.request)
                    .await?;
                let items = page.items.len() as u64;
                if self.authority.accept(page)? {
                    reconciler
                        .binding_holds
                        .retain_authority(&self.authority.items);
                    self.phase = Phase::Bindings;
                }
                return Ok(items);
            }
            Phase::Bindings => {
                let next = self.next_binding();
                if let Some(id) = next {
                    // Advance before I/O: cancellation must not pin this cursor.
                    // The guard retains protection until a successful fresh check.
                    self.binding_cursor = Some(id.clone());
                    let Some(hold) = reconciler
                        .binding_holds
                        .begin(&self.authority.items[&id], &self.inventory)
                    else {
                        return Ok(0);
                    };
                    self.pending_binding = Some(id.clone());
                    // A retained page is discovery only. Reread before adoption
                    // or recovery so a previous attempt's write is replay-safe.
                    let read = layer3_adapter::binding_diagnostics::measure(
                        layer3_adapter::binding_diagnostics::Stage::DurableRead,
                        reconciler.store.get_sandbox(&id),
                        super::diagnostics::store_outcome,
                    )
                    .await?;
                    let complete = if let Some(sandbox) = read {
                        hold.observe(&sandbox, &self.inventory);
                        sandbox
                            .validate()
                            .map_err(|error| corrupt(&error.to_string()))?;
                        reconciler
                            .reconcile_binding(&sandbox, &self.inventory, now)
                            .await?
                    } else {
                        true
                    };
                    if complete {
                        hold.complete();
                    } else {
                        hold.raced();
                    }
                    self.pending_binding = None;
                    return Ok(1);
                }
                self.authority = Authority::default();
                self.phase = Phase::Runtimes;
            }
            Phase::Runtimes => {
                if let Some(observed) = self.inventory.get(self.runtime_index) {
                    let held = reconciler.binding_holds.protects_runtime(observed);
                    if !held {
                        reconciler.reconcile_runtime(observed).await?;
                    }
                    self.runtime_index += 1;
                    return Ok(u64::from(!held));
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
                    let held = reconciler
                        .binding_holds
                        .contains(&claim.request.home.sandbox_id);
                    if !held {
                        reconciler.reconcile_native_claim(claim).await?;
                    }
                    self.claims.pop_front();
                    return Ok(u64::from(!held));
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
                    let held = reconciler.binding_holds.contains(&home.request.sandbox_id);
                    if !held {
                        reconciler.reconcile_home(home).await?;
                    }
                    self.homes.pop_front();
                    return Ok(u64::from(!held));
                }
                self.phase = Phase::PlatformSpend;
            }
            Phase::PlatformSpend => {
                // Once per sweep; a closure entered here alerts from the one
                // writer whose guarded update inserted it.
                self.platform_stop = reconciler.platform_spend_stop(now).await;
                self.phase = Phase::Policy;
                return Ok(1);
            }
            Phase::Policy => {
                self.explicit_updates_only = !self.platform_stop
                    && reconciler.automatic_pause_threshold(now).await?.is_none();
                self.phase = if !self.explicit_updates_only || reconciler.released.is_some() {
                    Phase::Candidates
                } else {
                    Phase::Complete
                };
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
                if reconciler.binding_holds.contains(id.as_str()) {
                    self.next_candidate();
                    return Ok(0);
                }
                if !self.explicit_updates_only {
                    if let Some(agents) = &reconciler.profile_agents {
                        if agents.reconcile_profile_agents(id).await.is_err() {
                            tracing::warn!(parent: None, phase = "profiles", reason = "unavailable", "automatic profile agent reconciliation failed");
                        }
                    }
                }
                self.phase = Phase::RuntimeUpdates;
                return Ok(1);
            }
            Phase::RuntimeUpdates => {
                if self.skip_held_candidate(reconciler) {
                    return Ok(0);
                }
                // A platform spend stop defers managed runtime updates: their
                // rotation starts a runtime even on a paused Sandbox, and an
                // outstanding one would keep the candidate from the pause.
                // They run again after the lift (decision 26).
                if reconciler.platform_spend_stop(now).await {
                    self.phase = Phase::IdlePause;
                    return Ok(0);
                }
                let id = self
                    .candidates
                    .front()
                    .expect("candidate is retained until admission finishes");
                if reconciler
                    .reconcile_runtime_update(id, now, !self.explicit_updates_only)
                    .await?
                {
                    self.next_candidate();
                } else {
                    self.phase = Phase::IdlePause;
                }
                return Ok(1);
            }
            Phase::IdlePause => {
                if self.skip_held_candidate(reconciler) {
                    return Ok(0);
                }
                // A sweep can yield or retry after policy changes. Never carry
                // enablement, grace or a threshold across an admission attempt.
                // A platform spend stop is a platform action, not inactivity.
                // Re-read per candidate (cached per interval): a lift mid-sweep
                // must not keep pausing with the platform notice.
                self.platform_stop = reconciler.platform_spend_stop(now).await;
                let (pause_after_seconds, cause) = if self.platform_stop {
                    (0, InactivityPauseCause::PlatformSpendStop)
                } else {
                    let Some(threshold) = reconciler.automatic_pause_threshold(now).await? else {
                        // An unrelated candidate must not hide a later explicit
                        // update when automatic inactivity maintenance is off.
                        self.explicit_updates_only = true;
                        self.next_candidate();
                        return Ok(0);
                    };
                    (threshold, InactivityPauseCause::Inactivity)
                };
                let sandbox_id = self
                    .candidates
                    .front()
                    .expect("candidate is retained until admission finishes")
                    .clone();
                let outcome = reconciler
                    .store
                    .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                        sandbox_id: sandbox_id.clone(),
                        requested_at: rfc3339_at(now),
                        pause_after_seconds,
                        auto_pause_enabled: true,
                        expected_policy_revision: None,
                        cause,
                    })
                    .await?;
                tracing::info!(parent: None,
                    sandbox_id = %sandbox_id,
                    outcome = ?outcome,
                    threshold_seconds = pause_after_seconds,
                    "automatic sandbox pause decision"
                );
                self.next_candidate();
                return Ok(1);
            }
            Phase::Complete => {}
        }
        Ok(0)
    }

    // A binding refresh can introduce a hold while this candidate is retained
    // midway through maintenance. Recheck at every admission boundary.
    fn skip_held_candidate(&mut self, reconciler: &SandboxReconciler) -> bool {
        if self
            .candidates
            .front()
            .is_some_and(|id| reconciler.binding_holds.contains(id.as_str()))
        {
            self.next_candidate();
            return true;
        }
        false
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

impl SandboxReconciler {
    async fn automatic_pause_threshold(
        &self,
        now: SystemTime,
    ) -> Result<Option<u64>, SandboxStoreError> {
        let policy = self.policy.current_sandbox_policy().await.map_err(|_| {
            SandboxStoreError::Unavailable("sandbox maintenance policy is unavailable".into())
        })?;
        if !self
            .policy
            .automatic_pause_allowed(&rfc3339_at(now))
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable(
                    "automatic pause enablement policy is unavailable".into(),
                )
            })?
        {
            tracing::debug!(parent: None,
                "automatic sandbox pause is disabled or within its enablement grace period"
            );
            return Ok(None);
        }
        Ok(Some(policy.inactivity_pause_after_seconds))
    }
}

#[cfg(test)]
#[path = "reconciliation_sweep_tests.rs"]
mod tests;

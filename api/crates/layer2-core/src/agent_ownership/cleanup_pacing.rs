//! Shared background cleanup pacing; exact-token custody remains in the coordinator.
use super::{Authority, OwnershipCoordinator, OwnershipError, OwnershipToken};
use crate::{
    retry_schedule::{Clock, Jitter, MonotonicClock, RandomJitter, RetryPolicy, RetrySchedule},
    AgentId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

struct State {
    schedule: RetrySchedule<AgentId>,
    bindings: BTreeMap<AgentId, Episode>,
    active: BTreeMap<AgentId, OwnershipToken>,
}

struct Episode {
    binding: [u8; 32],
    epoch: u64,
}

impl Episode {
    fn replaced_by(&self, authority: &Authority, binding: [u8; 32]) -> bool {
        authority.token.epoch >= self.epoch && self.binding != binding
    }
}

pub(super) struct CleanupPacing {
    enabled: bool,
    capacity: usize,
    state: Mutex<State>,
}

impl CleanupPacing {
    pub(super) fn new(enabled: bool, capacity: usize) -> Self {
        Self::with_timing(
            enabled,
            capacity,
            Arc::new(MonotonicClock),
            Arc::new(RandomJitter),
        )
    }

    fn with_timing(
        enabled: bool,
        capacity: usize,
        clock: Arc<dyn Clock>,
        jitter: Arc<dyn Jitter>,
    ) -> Self {
        Self {
            enabled,
            capacity,
            state: Mutex::new(State {
                schedule: RetrySchedule::new(
                    capacity,
                    RetryPolicy {
                        initial: Duration::from_secs(3),
                        maximum: Duration::from_secs(60),
                        retention: Duration::from_secs(300),
                    },
                    clock,
                    jitter,
                ),
                bindings: BTreeMap::new(),
                active: BTreeMap::new(),
            }),
        }
    }

    pub(super) fn begin<'a>(&'a self, authority: &'a Authority) -> Option<Attempt<'a>> {
        if !self.enabled {
            return Some(Attempt {
                pacing: None,
                authority,
                binding: None,
                acknowledged: false,
            });
        }
        let binding = authority.receiver_binding();
        let mut state = self.state.lock().unwrap();
        let id = &authority.token.agent_id;
        if state.active.contains_key(id) || state.active.len() >= self.capacity {
            return None;
        }
        if binding.is_some_and(|(binding, confirmed)| {
            confirmed
                && state
                    .bindings
                    .get(id)
                    .is_some_and(|old| old.replaced_by(authority, binding))
        }) {
            state.schedule.recovered(id);
            state.bindings.remove(id);
        }
        if !state.schedule.remaining(id).is_zero() {
            return None;
        }
        state.active.insert(id.clone(), authority.token.clone());
        Some(Attempt {
            pacing: Some(self),
            authority,
            binding: binding.map(|(binding, _)| binding),
            acknowledged: false,
        })
    }

    pub(super) fn deferred(&self, id: &AgentId) -> bool {
        if !self.enabled {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        state.active.contains_key(id) || !state.schedule.remaining(id).is_zero()
    }

    fn deferred_ids(&self) -> BTreeSet<AgentId> {
        if !self.enabled {
            return BTreeSet::new();
        }
        let mut state = self.state.lock().unwrap();
        let mut ids: BTreeSet<_> = state.schedule.deferred_keys().cloned().collect();
        ids.extend(state.active.keys().cloned());
        ids
    }

    pub(super) fn receiver_cleanup_completed(&self, id: &AgentId) {
        if self.enabled {
            let mut state = self.state.lock().unwrap();
            state.schedule.recovered(id);
            state.bindings.remove(id);
        }
    }

    fn verified_replacement(&self, authority: &Authority) -> bool {
        let Some((binding, true)) = authority.receiver_binding() else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        let id = &authority.token.agent_id;
        if !state.active.contains_key(id)
            && state
                .bindings
                .get(id)
                .is_some_and(|old| old.replaced_by(authority, binding))
        {
            state.schedule.recovered(id);
            state.bindings.remove(id);
            return true;
        }
        false
    }
}

pub(super) struct Attempt<'a> {
    pacing: Option<&'a CleanupPacing>,
    authority: &'a Authority,
    binding: Option<[u8; 32]>,
    acknowledged: bool,
}

impl Attempt<'_> {
    pub(super) fn confirmed(&mut self) {
        self.acknowledged = true;
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let Some(pacing) = self.pacing else {
            return;
        };
        let mut state = pacing.state.lock().unwrap();
        let token = &self.authority.token;
        let id = &token.agent_id;
        state.active.remove(id);
        if self.acknowledged {
            if state
                .bindings
                .get(id)
                .is_none_or(|episode| token.epoch >= episode.epoch)
            {
                state.schedule.recovered(id);
                state.bindings.remove(id);
            }
            return;
        }
        if let Some(binding) = self.binding {
            let (failures, delay) = state.schedule.failed(id.clone());
            state
                .bindings
                .entry(id.clone())
                .and_modify(|episode| episode.epoch = episode.epoch.max(token.epoch))
                .or_insert(Episode {
                    binding,
                    epoch: token.epoch,
                });
            let keys: Vec<_> = state.bindings.keys().cloned().collect();
            for key in keys {
                if !state.schedule.contains(&key) {
                    state.bindings.remove(&key);
                }
            }
            // Keep the existing pacing signal joinable without recording payloads.
            if failures >= 10 {
                drop(state);
                let fields = self.authority.receiver_correlation();
                super::receiver_diagnostics::with_root(|| {
                    tracing::warn!(target: super::receiver_diagnostics::TARGET, parent: None,
                        agent_sha256 = %fields.agent_sha256, sandbox_sha256 = fields.sandbox_sha256.as_deref(),
                        instance_sha256 = %fields.instance_sha256, receiver_sha256 = fields.receiver_sha256.as_deref(),
                        epoch = token.epoch, stage = "cleanup_pacing", reason = "unconfirmed",
                        failures, retry_after_ms = delay.as_millis() as u64,
                        intervention_required = true, "Ownership receiver cleanup still requires confirmation");
                });
            }
        }
    }
}

impl OwnershipCoordinator {
    /// Observe holds once per discovery pass, before the store applies its page limit.
    pub async fn background_cleanup_holds(&self) -> BTreeSet<AgentId> {
        let mut ids = self.cleanup_pacing.deferred_ids();
        if !ids.is_empty() && self.is_ready() {
            for authority in self.owned().await {
                let id = &authority.token.agent_id;
                if ids.contains(id)
                    && authority.is_current()
                    && self.cleanup_pacing.verified_replacement(&authority)
                {
                    ids.remove(id);
                }
            }
        }
        ids
    }

    /// Advisory only. Public control admission and shutdown drain never consult this.
    pub async fn background_cleanup_deferred(&self, id: &AgentId) -> bool {
        if !self.cleanup_pacing.deferred(id) {
            return false;
        }
        if let Some(authority) = self.local(id).await {
            self.cleanup_pacing.verified_replacement(&authority);
        }
        self.cleanup_pacing.deferred(id)
    }

    /// `false` retains custody for a later audit; it never means release completed.
    pub async fn handoff_background(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, OwnershipError> {
        if self.contains_failures() {
            return self
                .retirement_background(authority)
                .await
                .map(|result| result.is_some());
        }
        let Some(mut attempt) = self.cleanup_pacing.begin(authority) else {
            return Ok(false);
        };
        let result = self
            .handoff(authority, async {
                authority.revoke_transports().await?;
                attempt.confirmed();
                Ok(())
            })
            .await;
        if result.is_ok() {
            attempt.confirmed();
        }
        result.map(|()| true)
    }
}

#[cfg(test)]
mod tests;

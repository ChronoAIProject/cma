//! Advisory process-local suppression for a proved held runtime binding.
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use layer2_core::sandbox::{Sandbox, SandboxStatus};
use layer2_core::{AgentId, SandboxId};
use layer3_adapter::{AdapterError, RuntimeStartupProbe};
use tokio::time::Instant;

const MAX_HOLDS: usize = 128;
const MAX_REFRESH_PER_SWEEP: usize = 8;
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const PROBE_SEARCH_INTERVAL: Duration = Duration::from_secs(1);
const PROBE_RESELECTION_DELAY: Duration = Duration::from_secs(3);
const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(2 * 60 * 60);
const RESUME_WINDOW_MILLIS: u64 = 30_000;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Binding {
    runtime_id: String,
    generation: u64,
    process: String,
    status: SandboxStatus,
    credential_delivered: bool,
}

impl Binding {
    pub(super) fn current(sandbox: &Sandbox) -> Option<Self> {
        let runtime = sandbox.runtime.as_ref()?;
        let generation = runtime.provision_generation.filter(|value| *value > 0)?;
        let process = runtime.boot_id.as_ref().filter(|value| !value.is_empty())?;
        if runtime.runtime_id.is_empty() {
            return None;
        }
        Some(Self {
            runtime_id: runtime.runtime_id.clone(),
            generation,
            process: process.clone(),
            status: sandbox.status,
            credential_delivered: sandbox.credential.delivered,
        })
    }

    fn accepts(&self, sandbox_id: &SandboxId, probe: &RuntimeStartupProbe) -> bool {
        sandbox_id.as_str() == probe.sandbox_id
            && self.runtime_id == probe.runtime.as_str()
            && self.generation == probe.generation
    }
}

#[derive(Default)]
pub(super) struct StartupHolds {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<SandboxId, Entry>,
    next_token: u64,
    probe_turn: bool,
}

struct Entry {
    binding: Binding,
    observed_at: Instant,
    refresh_at: Instant,
    state: EntryState,
}

enum EntryState {
    Closed {
        probe_at: Instant,
        reselect_at: Instant,
        next_search_at: Instant,
        permit: Option<u64>,
    },
    Resuming {
        since: Instant,
    },
}

pub(super) struct Refresh {
    pub(super) id: SandboxId,
    binding: Binding,
    observed_at: Instant,
}

pub(super) struct Discovery {
    pub(super) excluded: BTreeSet<SandboxId>,
    pub(super) probe: Option<SandboxId>,
}

pub(super) enum Admission<'a> {
    Proceed,
    Probe(ProbePermit<'a>),
    Hold,
}

pub(super) struct ProbePermit<'a> {
    owner: &'a StartupHolds,
    sandbox: SandboxId,
    agent: AgentId,
    token: u64,
}

impl StartupHolds {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| {
            tracing::error!("agent startup hold cache was poisoned");
            error.into_inner()
        })
    }

    pub(super) fn pending(&self) -> bool {
        !self.lock().entries.is_empty()
    }

    pub(super) fn holds(&self, id: &SandboxId) -> bool {
        self.lock().entries.contains_key(id)
    }

    pub(super) fn observe<T>(
        &self,
        sandbox: &Sandbox,
        started: Instant,
        result: &Result<T, AdapterError>,
        now: Instant,
    ) {
        let Some(binding) = Binding::current(sandbox) else {
            return;
        };
        match result {
            Ok(_) => self.observe_serving(&sandbox.id, &binding, started, now),
            Err(AdapterError::RuntimeStartupHold(failure)) => {
                if let Some(probe) = failure
                    .probe()
                    .filter(|probe| binding.accepts(&sandbox.id, probe))
                {
                    self.observe_hold(&sandbox.id, binding, started, probe.next_probe_at, now);
                }
            }
            Err(_) => {}
        }
    }

    fn observe_hold(
        &self,
        id: &SandboxId,
        binding: Binding,
        observed_at: Instant,
        probe_at: Instant,
        now: Instant,
    ) {
        let mut state = self.lock();
        if let Some(entry) = state.entries.get_mut(id) {
            if entry.observed_at > observed_at {
                return;
            }
            if entry.binding == binding {
                if let EntryState::Closed {
                    probe_at: previous, ..
                } = &mut entry.state
                {
                    *previous = (*previous).max(probe_at);
                    entry.observed_at = observed_at;
                    return;
                }
            }
        } else if state.entries.len() >= MAX_HOLDS {
            metrics::counter!("cma_agent_runtime_startup_hold_overflow_total").increment(1);
            return;
        }
        state.entries.insert(
            id.clone(),
            Entry {
                binding,
                observed_at,
                refresh_at: now + REFRESH_INTERVAL,
                state: EntryState::Closed {
                    probe_at,
                    reselect_at: now,
                    next_search_at: now,
                    permit: None,
                },
            },
        );
        state.publish_active();
        tracing::warn!(parent: None, sandbox_id = %id, "Agent sibling work held after runtime startup failures");
    }

    fn observe_serving(&self, id: &SandboxId, binding: &Binding, started: Instant, now: Instant) {
        let mut state = self.lock();
        if let Some(entry) = state
            .entries
            .get_mut(id)
            .filter(|entry| entry.binding == *binding && started >= entry.observed_at)
        {
            if matches!(entry.state, EntryState::Closed { .. }) {
                entry.resume(binding.clone(), now);
                state.publish_active();
                tracing::warn!(parent: None, sandbox_id = %id, "Agent sibling work released after serving proof");
            }
        }
    }

    pub(super) fn refresh_candidates(&self, now: Instant) -> Vec<Refresh> {
        let mut state = self.lock();
        let previous = state.entries.len();
        state.entries.retain(|_, entry| entry.keep(now));
        if state.entries.len() != previous {
            state.publish_active();
        }
        let mut candidates: Vec<_> = state
            .entries
            .iter_mut()
            .filter(|(_, entry)| entry.refresh_at <= now)
            .collect();
        candidates.sort_by_key(|(_, entry)| entry.refresh_at);
        candidates
            .into_iter()
            .take(MAX_REFRESH_PER_SWEEP)
            .map(|(id, entry)| {
                entry.refresh_at = now + REFRESH_INTERVAL;
                Refresh {
                    id: id.clone(),
                    binding: entry.binding.clone(),
                    observed_at: entry.observed_at,
                }
            })
            .collect()
    }

    pub(super) fn refreshed(&self, refresh: Refresh, current: Option<Binding>, now: Instant) {
        let mut state = self.lock();
        if state.entries.get(&refresh.id).is_some_and(|entry| {
            entry.binding == refresh.binding && entry.observed_at == refresh.observed_at
        }) {
            state.refresh_binding(&refresh.id, current, now);
        }
    }

    pub(super) fn discovery(&self, now: Instant, limit: u16) -> Discovery {
        let mut state = self.lock();
        let excluded = state
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, EntryState::Closed { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        let mut probe = state
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .wake_at()
                    .filter(|wake| *wake <= now)
                    .map(|wake| (wake, id))
            })
            .min_by_key(|(wake, _)| *wake)
            .map(|(_, id)| id.clone());
        // A one-item page alternates between the directed probe and ordinary
        // work when both lanes need capacity.
        if limit == 1 && probe.is_some() {
            state.probe_turn = !state.probe_turn;
            if !state.probe_turn {
                probe = None;
            }
        }
        if let Some(id) = &probe {
            if let Some(Entry {
                state: EntryState::Closed { next_search_at, .. },
                ..
            }) = state.entries.get_mut(id)
            {
                *next_search_at = now + PROBE_SEARCH_INTERVAL;
            }
        }
        Discovery { excluded, probe }
    }

    pub(super) fn next_probe(&self) -> Option<Instant> {
        self.lock()
            .entries
            .values()
            .filter_map(Entry::wake_at)
            .min()
    }

    pub(super) fn admit(
        &self,
        id: &SandboxId,
        agent: &AgentId,
        binding: Option<Binding>,
        existing: Option<&ProbePermit<'_>>,
        now: Instant,
        may_start_probe: bool,
    ) -> Admission<'_> {
        let mut state = self.lock();
        state.refresh_binding(id, binding, now);
        let token = state.next_token;
        let Some(entry) = state.entries.get_mut(id) else {
            return Admission::Proceed;
        };
        match &mut entry.state {
            EntryState::Resuming { since } => {
                if now >= *since + resume_delay(agent) {
                    Admission::Proceed
                } else {
                    Admission::Hold
                }
            }
            EntryState::Closed {
                probe_at,
                reselect_at,
                permit,
                ..
            } => {
                if existing.is_some_and(|held| {
                    held.sandbox == *id && held.agent == *agent && Some(held.token) == *permit
                }) {
                    return Admission::Proceed;
                }
                if !may_start_probe || permit.is_some() || now < (*probe_at).max(*reselect_at) {
                    return Admission::Hold;
                }
                *permit = Some(token);
                state.next_token = token.wrapping_add(1);
                Admission::Probe(ProbePermit {
                    owner: self,
                    sandbox: id.clone(),
                    agent: agent.clone(),
                    token,
                })
            }
        }
    }
}

impl State {
    fn publish_active(&self) {
        let count = self
            .entries
            .values()
            .filter(|entry| matches!(entry.state, EntryState::Closed { .. }))
            .count();
        metrics::gauge!("cma_agent_runtime_startup_holds_active").set(count as f64);
    }

    fn refresh_binding(&mut self, id: &SandboxId, current: Option<Binding>, now: Instant) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if let Some(current) = current {
            if entry.binding != current {
                entry.resume(current, now);
                self.publish_active();
                tracing::warn!(parent: None, sandbox_id = %id, "Agent sibling work released after runtime binding changed");
            }
        } else {
            self.entries.remove(id);
            self.publish_active();
        }
    }
}

impl Entry {
    fn wake_at(&self) -> Option<Instant> {
        match self.state {
            EntryState::Closed {
                probe_at,
                reselect_at,
                next_search_at,
                permit: None,
            } => Some(probe_at.max(reselect_at).max(next_search_at)),
            _ => None,
        }
    }

    fn keep(&self, now: Instant) -> bool {
        match self.state {
            EntryState::Closed { permit, .. } => {
                permit.is_some() || now < self.observed_at + MAX_OBSERVATION_AGE
            }
            EntryState::Resuming { since } => {
                now < since + Duration::from_millis(RESUME_WINDOW_MILLIS)
            }
        }
    }

    fn resume(&mut self, binding: Binding, now: Instant) {
        self.binding = binding;
        self.observed_at = now;
        self.state = EntryState::Resuming { since: now };
    }
}

impl Drop for ProbePermit<'_> {
    fn drop(&mut self) {
        let mut state = self.owner.lock();
        if let Some(entry) = state.entries.get_mut(&self.sandbox) {
            if let EntryState::Closed {
                reselect_at,
                permit,
                ..
            } = &mut entry.state
            {
                if *permit == Some(self.token) {
                    *permit = None;
                    *reselect_at = Instant::now() + PROBE_RESELECTION_DELAY;
                }
            }
        }
    }
}

fn resume_delay(agent: &AgentId) -> Duration {
    let mut hasher = DefaultHasher::new();
    agent.hash(&mut hasher);
    Duration::from_millis(hasher.finish() % RESUME_WINDOW_MILLIS)
}

#[cfg(test)]
mod tests;

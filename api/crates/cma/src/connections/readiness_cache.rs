//! Bounded presentation evidence. Send checks never read this cache.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use layer2_core::{
    trigger_domain::{ServiceConnectionBroker, ServiceRequirement, ServiceRequirementState},
    ServiceId, SubjectId,
};
use tokio::{sync::Semaphore, time::Instant};

const LIFETIME: Duration = Duration::from_secs(30);
const CAPACITY: usize = 4096;
const MAX_SERVICES: usize = 64;
const REFRESHES: usize = 4;
type Key = (SubjectId, ServiceId);

struct Verified {
    at: Instant,
    checked_at: String,
}
struct Entry {
    generation: u64,
    verified: Option<Verified>,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<Key, Entry>,
    generation: u64,
    refreshing: BTreeSet<SubjectId>,
}

pub(super) struct ReadinessCache {
    state: Mutex<State>,
    slots: Arc<Semaphore>,
}

pub(super) struct Ticket(Vec<(Key, u64)>);
pub(super) struct Cached {
    pub checked_at: String,
    pub valid_for_ms: u64,
}

impl Default for ReadinessCache {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            slots: Arc::new(Semaphore::new(REFRESHES)),
        }
    }
}

impl ReadinessCache {
    #[cfg(test)]
    pub async fn idle(&self) {
        drop(self.slots.acquire_many(REFRESHES as u32).await.unwrap());
    }
    pub fn begin(&self, owner: &SubjectId, services: &[ServiceId]) -> Ticket {
        if services.len() > MAX_SERVICES {
            return Ticket(Vec::new());
        }
        let mut state = self.state.lock().unwrap();
        let mut ticket = Vec::new();
        for service in services {
            let Some(generation) = state.generation.checked_add(1) else {
                return Ticket(Vec::new());
            };
            state.generation = generation;
            let key = (owner.clone(), service.clone());
            if !state.entries.contains_key(&key) && state.entries.len() >= CAPACITY {
                state.entries.pop_first();
            }
            let entry = state.entries.entry(key.clone()).or_insert(Entry {
                generation,
                verified: None,
            });
            entry.generation = generation;
            ticket.push((key, generation));
        }
        Ticket(ticket)
    }

    pub fn finish(&self, ticket: Ticket, rows: Option<&[ServiceRequirement]>) {
        let mut state = self.state.lock().unwrap();
        let checked_at = layer2_core::now_rfc3339();
        for (index, (key, generation)) in ticket.0.into_iter().enumerate() {
            let Some(entry) = state
                .entries
                .get_mut(&key)
                .filter(|entry| entry.generation == generation)
            else {
                continue;
            };
            entry.verified = rows
                .and_then(|rows| rows.get(index))
                .filter(|row| {
                    row.service == key.1 && row.state == ServiceRequirementState::Satisfied
                })
                .map(|_| Verified {
                    at: Instant::now(),
                    checked_at: checked_at.clone(),
                });
        }
    }

    pub fn get(&self, owner: &SubjectId, services: &[ServiceId]) -> Option<Cached> {
        if services.is_empty() || services.len() > MAX_SERVICES {
            return None;
        }
        let state = self.state.lock().unwrap();
        let mut oldest: Option<&Verified> = None;
        for service in services {
            let verified = state
                .entries
                .get(&(owner.clone(), service.clone()))?
                .verified
                .as_ref()?;
            if verified.at.elapsed() >= LIFETIME {
                return None;
            }
            if oldest.is_none_or(|oldest| verified.at < oldest.at) {
                oldest = Some(verified);
            }
        }
        let oldest = oldest?;
        Some(Cached {
            checked_at: oldest.checked_at.clone(),
            valid_for_ms: LIFETIME.saturating_sub(oldest.at.elapsed()).as_millis() as u64,
        })
    }

    pub fn invalidate(&self, owner: &SubjectId) {
        self.state
            .lock()
            .unwrap()
            .entries
            .retain(|(subject, _), _| subject != owner);
    }

    pub fn refresh(
        self: &Arc<Self>,
        broker: Arc<dyn ServiceConnectionBroker>,
        owner: SubjectId,
        services: Vec<ServiceId>,
    ) {
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return;
        };
        if !self.state.lock().unwrap().refreshing.insert(owner.clone()) {
            return;
        }
        let ticket = self.begin(&owner, &services);
        let guard = Refresh {
            cache: self.clone(),
            owner: owner.clone(),
        };
        // Bounded, single-flight work survives the Open response. In particular,
        // returning a cached report must not cancel OAuth credential custody.
        tokio::spawn(async move {
            let _permit = permit;
            let _guard = guard;
            let result = broker.requirements(&owner, &services).await;
            let rows = result.as_ref().ok().filter(|rows| exact(&services, rows));
            _guard.cache.finish(ticket, rows.map(Vec::as_slice));
        });
    }
}

pub(super) fn exact(services: &[ServiceId], rows: &[ServiceRequirement]) -> bool {
    services.len() == rows.len() && services.iter().eq(rows.iter().map(|row| &row.service))
}

struct Refresh {
    cache: Arc<ReadinessCache>,
    owner: SubjectId,
}
impl Drop for Refresh {
    fn drop(&mut self) {
        self.cache
            .state
            .lock()
            .unwrap()
            .refreshing
            .remove(&self.owner);
    }
}

#[cfg(test)]
#[path = "readiness_cache_tests.rs"]
mod tests;

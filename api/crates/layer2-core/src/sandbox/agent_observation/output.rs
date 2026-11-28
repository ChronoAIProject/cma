//! Immutable output evidence shares the source cache and per-Agent load lock.
//! A cache hit supplies data only; callers still perform their own release fence.
use super::*;
use crate::sandbox::{store::SandboxStoreError, Agent};

type GenerationReply = oneshot::Sender<Vec<u8>>;

#[derive(Default)]
pub(super) struct GenerationWaiters(Vec<GenerationCohort>);

struct GenerationCohort {
    agent: Agent,
    replies: Vec<GenerationReply>,
}

impl GenerationWaiters {
    fn join(&mut self, agent: &Agent) -> oneshot::Receiver<Vec<u8>> {
        let mut queued = 0;
        self.0.retain_mut(|cohort| {
            cohort.replies.retain(|reply| !reply.is_closed());
            queued += cohort.replies.len();
            !cohort.replies.is_empty()
        });
        let (reply, receive) = oneshot::channel();
        if queued < MAX_OBSERVATION_COHORT {
            if let Some(cohort) = self.0.iter_mut().find(|cohort| &cohort.agent == agent) {
                cohort.replies.push(reply);
            } else {
                self.0.push(GenerationCohort {
                    agent: agent.clone(),
                    replies: vec![reply],
                });
            }
        }
        receive
    }

    fn take(&mut self, agent: &Agent) -> Vec<GenerationReply> {
        self.0
            .iter()
            .position(|cohort| &cohort.agent == agent)
            .map(|index| self.0.swap_remove(index).replies)
            .unwrap_or_default()
    }
}

pub(in crate::sandbox) struct OutputFlight<'a> {
    service: &'a AgentObservationService,
    authority: Arc<AgentObservationAuthority>,
    key: Vec<u8>,
    previous: Vec<u8>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}
impl AgentObservationService {
    pub(in crate::sandbox) async fn output_flight(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<OutputFlight<'_>>, SandboxStoreError> {
        if durable_read_fallback() {
            return Ok(None);
        }
        let lane = self
            .registry
            .lock()
            .unwrap()
            .lanes
            .get(agent.id.as_str())
            .map(|lane| {
                (
                    lane.authority.clone(),
                    lane.flight.clone(),
                    lane.output_waiters.clone(),
                )
            });
        let Some((authority, flight, waiters)) = lane else {
            return Ok(None);
        };
        if !authority.current() || !self.store.ready() {
            return Err(SandboxStoreError::Unavailable("read custody ended".into()));
        }
        let mut reply = waiters.lock().unwrap().join(agent);
        let guard = flight.lock_owned().await;
        if !authority.current() || !self.store.ready() {
            return Err(SandboxStoreError::Unavailable("read custody ended".into()));
        }
        let generation = match reply.try_recv() {
            Ok(generation) => generation,
            Err(_) => {
                // Only readers already waiting for this exact Agent may share
                // the snapshot. Later arrivals must check the store again.
                let cohort = waiters.lock().unwrap().take(agent);
                let Some(generation) = self.store.output_generation(agent).await? else {
                    return Ok(None);
                };
                for reply in cohort {
                    let _ = reply.send(generation.clone());
                }
                generation
            }
        };
        let key = serde_json::to_vec(&(
            2_u8,
            &agent.id,
            &agent.created_at,
            agent.revision,
            agent.conversation.runtime_generation,
            agent.conversation.last_seq,
            generation,
            turn,
        ))
        .expect("output source coordinates serialize");
        let previous = serde_json::to_vec(&(4_u8, &agent.id, &agent.created_at, turn))
            .expect("output identity serializes");
        Ok(Some(OutputFlight {
            service: self,
            authority,
            key,
            previous,
            _guard: guard,
        }))
    }
}
impl OutputFlight<'_> {
    pub(in crate::sandbox) fn identity(&self) -> Vec<u8> {
        self.key.clone()
    }
    pub(in crate::sandbox) fn previous_window(
        &self,
    ) -> Option<Arc<crate::sandbox::store::AgentResponseLogWindow>> {
        self.service.cache.lock().unwrap().get(&self.previous)
    }
    pub(in crate::sandbox) fn remember_window(
        &self,
        window: crate::sandbox::store::AgentResponseLogWindow,
    ) {
        let Ok(bytes) = serde_json::to_vec(&window) else {
            return;
        };
        if bytes.len() <= MAX_PAGE_BYTES && self.authority.current() && self.service.store.ready() {
            self.service.cache.lock().unwrap().insert(
                self.previous.clone(),
                Arc::new(window),
                bytes.len(),
            );
        }
    }
    fn key(&self, coordinate: &[u8]) -> Vec<u8> {
        let mut key = self.key.clone();
        key.extend_from_slice(coordinate);
        key
    }
    pub(in crate::sandbox) fn get<T: Send + Sync + 'static>(
        &self,
        coordinate: &[u8],
    ) -> Option<Arc<T>> {
        self.service
            .cache
            .lock()
            .unwrap()
            .get(&self.key(coordinate))
    }
    pub(in crate::sandbox) fn insert<T: serde::Serialize + Send + Sync + 'static>(
        &self,
        coordinate: &[u8],
        value: T,
    ) {
        if !self.authority.current() || !self.service.store.ready() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return;
        };
        if bytes.len() <= MAX_PAGE_BYTES {
            self.service.cache.lock().unwrap().insert(
                self.key(coordinate),
                Arc::new(value),
                bytes.len(),
            );
        }
    }
}

impl AgentObservationService {
    pub(in crate::sandbox) fn memoize_verified_fold<T>(
        &self,
        fingerprint: [u8; 32],
        build: impl FnOnce() -> T,
    ) -> T
    where
        T: Clone + serde::Serialize + Send + Sync + 'static,
    {
        let mut key = vec![3];
        key.extend_from_slice(std::any::type_name::<T>().as_bytes());
        key.extend_from_slice(&fingerprint);
        // The fold is synchronous and pure: serialize misses under this lock so
        // concurrent readers cannot duplicate the same immutable fold work.
        let mut cache = self.cache.lock().unwrap();
        if let Some(value) = cache.get::<T>(&key) {
            return (*value).clone();
        }
        let value = build();
        if let Ok(bytes) = serde_json::to_vec(&value) {
            if bytes.len() <= MAX_PAGE_BYTES {
                cache.insert(key, Arc::new(value.clone()), bytes.len());
            }
        }
        value
    }
}

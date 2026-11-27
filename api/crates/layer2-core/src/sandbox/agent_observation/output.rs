//! Immutable output evidence shares the source cache and per-Agent load lock.
//! A cache hit supplies data only; callers still perform their own release fence.
use super::*;
use crate::sandbox::{store::SandboxStoreError, Agent};

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
            .map(|lane| (lane.authority.clone(), lane.flight.clone()));
        let Some((authority, flight)) = lane else {
            return Ok(None);
        };
        let guard = flight.lock_owned().await;
        if !authority.current() || !self.store.ready() {
            return Err(SandboxStoreError::Unavailable("read custody ended".into()));
        }
        let Some(generation) = self.store.output_generation(agent).await? else {
            return Ok(None);
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

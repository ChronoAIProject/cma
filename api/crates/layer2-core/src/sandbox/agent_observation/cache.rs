//! Only verified immutable source entries live here. Authorization and redaction
//! are applied by each reader after retrieval, including cache hits.
use super::*;
use crate::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use std::{any::Any, collections::VecDeque};

pub const MAX_CACHED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PAGES: usize = 256;
pub(in crate::sandbox) struct SourcePage {
    pub entries: Option<Arc<Vec<AgentLogEntry>>>,
    // Retain serialization through the oversized visitor fallback as well.
    _flight: Option<tokio::sync::OwnedMutexGuard<()>>,
}
#[derive(Default)]
pub(super) struct Cache {
    pages: VecDeque<(Vec<u8>, Arc<dyn Any + Send + Sync>, usize)>,
    bytes: usize,
}
impl Cache {
    pub(super) fn get<T: Any + Send + Sync>(&mut self, key: &[u8]) -> Option<Arc<T>> {
        let index = self.pages.iter().position(|entry| entry.0 == key)?;
        let entry = self.pages.remove(index)?;
        let result = entry.1.clone();
        self.pages.push_back(entry);
        result.downcast().ok()
    }
    pub(super) fn insert<T: Any + Send + Sync>(
        &mut self,
        key: Vec<u8>,
        page: Arc<T>,
        bytes: usize,
    ) {
        if let Some(index) = self.pages.iter().position(|entry| entry.0 == key) {
            if let Some((_, _, size)) = self.pages.remove(index) {
                self.bytes -= size;
            }
        }
        while self.bytes + bytes > MAX_CACHED_BYTES || self.pages.len() >= MAX_PAGES {
            let Some((_, _, size)) = self.pages.pop_front() else {
                break;
            };
            self.bytes -= size;
        }
        self.bytes += bytes;
        self.pages.push_back((key, page, bytes));
    }
}

impl AgentObservationService {
    pub(in crate::sandbox) async fn source_page<F, Fut>(
        &self,
        agent: &Agent,
        coordinates: (u64, u64, u16, bool),
        load: F,
    ) -> Result<SourcePage, SandboxStoreError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<Vec<AgentLogEntry>>, SandboxStoreError>>,
    {
        let lane = self
            .registry
            .lock()
            .unwrap()
            .lanes
            .get(agent.id.as_str())
            .map(|lane| (lane.authority.clone(), lane.flight.clone()));
        let Some((authority, flight)) = lane else {
            return Ok(SourcePage {
                entries: load().await?.map(Arc::new),
                _flight: None,
            });
        };
        let flight = flight.lock_owned().await;
        if !self.store.ready() || !authority.current() {
            return Err(SandboxStoreError::Unavailable("read custody ended".into()));
        }
        let generation = self.store.source_generation(agent).await?;
        let key = generation.map(|generation| {
            serde_json::to_vec(&(
                1_u8,
                &agent.id,
                &agent.created_at,
                agent.revision,
                agent.conversation.runtime_generation,
                agent.conversation.last_seq,
                generation,
                coordinates,
            ))
            .expect("source coordinates serialize")
        });
        if let Some(key) = &key {
            if let Some(page) = self.cache.lock().unwrap().get(key) {
                return Ok(SourcePage {
                    entries: Some(page),
                    _flight: Some(flight),
                });
            }
        }
        let Some(page) = load().await? else {
            return Ok(SourcePage {
                entries: None,
                _flight: Some(flight),
            });
        };
        let bytes = page
            .iter()
            .try_fold(0, |total, entry| {
                serde_json::to_vec(entry).map(|value| total + value.len())
            })
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let page = Arc::new(page);
        if let Some(key) = key {
            if bytes <= MAX_PAGE_BYTES
                && page.len() <= 201
                && authority.current()
                && self.store.ready()
            {
                self.cache.lock().unwrap().insert(key, page.clone(), bytes);
            }
        }
        Ok(SourcePage {
            entries: Some(page),
            _flight: Some(flight),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_verified_data_reclaims_the_previous_weight_and_keeps_the_latest_value() {
        let mut cache = Cache::default();
        cache.insert(vec![1], Arc::new(vec![7u64]), 128);
        cache.insert(vec![1], Arc::new(vec![9u64]), 256);
        assert_eq!(cache.bytes, 256);
        assert_eq!(cache.pages.len(), 1);
        assert_eq!(*cache.get::<Vec<u64>>(&[1]).unwrap(), vec![9]);
        assert!(cache.get::<Vec<String>>(&[1]).is_none());
    }
    #[test]
    fn cache_byte_budget_evicts_the_oldest_verified_payload() {
        let page = Arc::new(vec![AgentLogEntry {
            agent_id: AgentId("fixture".into()),
            seq: 1,
            appended_at: "2026-11-16T00:00:00Z".into(),
            kind: crate::sandbox::AgentLogKind::AgentMessage {
                content: "x".repeat(MAX_PAGE_BYTES - 1024),
            },
        }]);
        let bytes = serde_json::to_vec(&page[0]).unwrap().len();
        let mut cache = Cache::default();
        for key in 0..17 {
            cache.insert(vec![key], page.clone(), bytes);
        }
        assert!(cache.bytes <= MAX_CACHED_BYTES);
        assert!(cache.get::<Vec<AgentLogEntry>>(&[0]).is_none());
        assert_eq!(*cache.get::<Vec<AgentLogEntry>>(&[16]).unwrap(), *page);
    }
}

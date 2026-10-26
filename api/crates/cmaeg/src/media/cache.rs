//! A byte-bounded per-pod cache of derived media (decoded images, extracted
//! text, transcripts), so repeated reads of one attachment do the work once.
//!
//! Entries are weighted by their bytes and evicted least recently used first,
//! and none outlives the 30-minute event_ref that authorizes reading it.
use crate::telemetry::names;
use cmaeg_core::ids::{ChannelId, EventId};
use moka::{policy::EvictionPolicy, sync::Cache};
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc, time::Duration};

/// The event_ref lifetime.
const TIME_TO_LIVE: Duration = Duration::from_secs(30 * 60);

/// One derived artifact of one attachment of one event on one channel.
///
/// A SHA-256 over the length-prefixed fields, so no field boundary is
/// ambiguous. Paged artifacts are cached whole and callers select the page.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    pub fn new(
        channel_id: &ChannelId,
        event_id: &EventId,
        activity_sha256: &str,
        index: u8,
        artifact: &'static str,
    ) -> Self {
        let mut hasher = Sha256::new();
        for field in [
            channel_id.as_str().as_bytes(),
            event_id.as_str().as_bytes(),
            activity_sha256.as_bytes(),
            &[index],
            artifact.as_bytes(),
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field);
        }
        Self(hasher.finalize().into())
    }
}

/// Only a prefix of the digest, enough to correlate entries.
impl fmt::Debug for CacheKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "CacheKey(")?;
        for byte in &self.0[..6] {
            write!(formatter, "{byte:02x}")?;
        }
        write!(formatter, ")")
    }
}

/// The bytes a cached value holds, which is what the cache budget counts.
pub trait Weighted {
    fn weight(&self) -> u64;
}

/// The derived-media cache; with a zero budget every lookup misses.
pub struct DerivedCache<T: Weighted + Send + Sync + 'static> {
    cache: Option<Cache<CacheKey, Arc<T>>>,
}

impl<T: Weighted + Send + Sync + 'static> Clone for DerivedCache<T> {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
        }
    }
}

impl<T: Weighted + Send + Sync + 'static> DerivedCache<T> {
    pub fn new(cache_bytes: u64) -> Self {
        let cache = (cache_bytes > 0).then(|| {
            Cache::builder()
                .max_capacity(cache_bytes)
                .weigher(|_key, value: &Arc<T>| u32::try_from(value.weight()).unwrap_or(u32::MAX))
                .time_to_live(TIME_TO_LIVE)
                .eviction_policy(EvictionPolicy::lru())
                .build()
        });
        Self { cache }
    }

    /// A cached value returns derived content without asking who may see it,
    /// so callers consult the cache only *after* authorizing the request.
    pub fn get(&self, key: &CacheKey) -> Option<Arc<T>> {
        self.cache.as_ref()?.get(key)
    }

    pub fn insert(&self, key: CacheKey, value: Arc<T>) {
        let Some(cache) = &self.cache else {
            return;
        };
        cache.insert(key, value);
        // Eviction is applied lazily; settle it so the budget holds and the
        // gauge reports what is actually retained.
        cache.run_pending_tasks();
        metrics::gauge!(names::MEDIA_CACHE_BYTES).set(cache.weighted_size() as f64);
    }

    /// Weighted bytes currently retained.
    pub fn weighted_size(&self) -> u64 {
        self.cache.as_ref().map_or(0, Cache::weighted_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(event: &str, index: u8, artifact: &'static str) -> CacheKey {
        CacheKey::new(
            &ChannelId::new("cmaeg1.ch.cache").unwrap(),
            &event.parse().unwrap(),
            "sha",
            index,
            artifact,
        )
    }

    #[test]
    fn keys_separate_every_field_unambiguously() {
        assert_eq!(key("e1", 0, "text"), key("e1", 0, "text"));
        assert_ne!(key("e1", 0, "text"), key("e1", 1, "text"));
        assert_ne!(key("e1", 0, "text"), key("e1", 0, "image"));
        assert_ne!(key("e1", 0, "text"), key("e2", 0, "text"));
        // Moving bytes across a field boundary changes the key.
        let channel = ChannelId::new("cmaeg1.ch.cache").unwrap();
        assert_ne!(
            CacheKey::new(&channel, &"ab".parse().unwrap(), "c", 0, "x"),
            CacheKey::new(&channel, &"a".parse().unwrap(), "bc", 0, "x"),
        );
        assert_eq!(format!("{:?}", key("e1", 0, "text")).len(), 22);
    }
}

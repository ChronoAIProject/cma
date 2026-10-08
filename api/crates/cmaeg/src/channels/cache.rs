//! Bounded process-local channel records, with a limited stale window.
use super::resolver::ResolvedChannel;
use cmaeg_core::problem::Problem;
use dashmap::DashMap;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum CacheEntry {
    Present(Arc<ResolvedChannel>, Instant),
    Negative(Problem, Instant),
}
impl CacheEntry {
    fn expires_at(&self) -> Instant {
        match self {
            Self::Present(_, at) | Self::Negative(_, at) => *at,
        }
    }
    fn loaded_at(&self, negative_ttl: Duration) -> Instant {
        match self {
            Self::Present(channel, _) => channel.loaded_at,
            Self::Negative(_, at) => *at - negative_ttl,
        }
    }
}

pub(crate) enum Lookup {
    Fresh(Arc<ResolvedChannel>),
    Stale(Arc<ResolvedChannel>),
    Negative(Problem),
    Miss,
}

pub struct ChannelCache {
    entries: DashMap<String, CacheEntry>,
    ttl: Duration,
    negative_ttl: Duration,
    max_entries: usize,
    insertion: Mutex<()>,
}
impl ChannelCache {
    pub fn new(config: &crate::ChannelCacheConfig) -> Self {
        Self {
            entries: DashMap::new(),
            ttl: config.ttl,
            negative_ttl: config.negative_ttl,
            max_entries: config.max_entries.max(1),
            insertion: Mutex::new(()),
        }
    }
    pub(crate) fn lookup(&self, id: &str) -> Lookup {
        let Some(entry) = self.entries.get(id) else {
            return Lookup::Miss;
        };
        let now = Instant::now();
        match entry.value() {
            CacheEntry::Present(channel, expires) if *expires > now => {
                Lookup::Fresh(channel.clone())
            }
            CacheEntry::Present(channel, expires) if now.duration_since(*expires) < self.ttl => {
                Lookup::Stale(channel.clone())
            }
            CacheEntry::Negative(problem, expires) if *expires > now => {
                Lookup::Negative(problem.clone())
            }
            _ => Lookup::Miss,
        }
    }
    pub(crate) fn insert(&self, id: String, entry: CacheEntry) {
        let _guard = self.insertion.lock().unwrap();
        let before = self.entries.len();
        self.entries.insert(id, entry);
        if self.entries.len() > self.max_entries {
            let now = Instant::now();
            self.entries.retain(|_, value| value.expires_at() > now);
        }
        while self.entries.len() > self.max_entries {
            let victim = self
                .entries
                .iter()
                .take(32)
                .min_by_key(|entry| entry.loaded_at(self.negative_ttl))
                .map(|entry| entry.key().clone());
            if let Some(victim) = victim {
                self.entries.remove(&victim);
            } else {
                break;
            }
        }
        metrics::gauge!(crate::telemetry::names::CHANNELS_CACHED)
            .increment(self.entries.len() as f64 - before as f64);
    }
    pub(crate) fn invalidate(&self, id: &str) -> usize {
        let _guard = self.insertion.lock().unwrap();
        if self.entries.remove(id).is_some() {
            metrics::gauge!(crate::telemetry::names::CHANNELS_CACHED).decrement(1);
            1
        } else {
            0
        }
    }
    /// Management mutations scan the bounded cache instead of maintaining an
    /// alias index on every insertion, replacement and eviction.
    pub(crate) fn invalidate_where(&self, mut matches: impl FnMut(&CacheEntry) -> bool) -> usize {
        let _guard = self.insertion.lock().unwrap();
        let before = self.entries.len();
        self.entries.retain(|_, entry| !matches(entry));
        let removed = before - self.entries.len();
        metrics::gauge!(crate::telemetry::names::CHANNELS_CACHED).decrement(removed as f64);
        removed
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
impl Drop for ChannelCache {
    fn drop(&mut self) {
        metrics::gauge!(crate::telemetry::names::CHANNELS_CACHED)
            .decrement(self.entries.len() as f64);
    }
}

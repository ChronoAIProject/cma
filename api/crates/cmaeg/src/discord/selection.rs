//! Bounded runtime selection shared by discovery, native admission and owner views.
use crate::NativeDiscordConfig;
use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

pub const MAX_RECORDS: usize = 64;
#[derive(Clone, Default)]
pub struct Selection {
    enabled: bool,
    discovery: bool,
    records: Arc<RwLock<BTreeSet<String>>>,
    status: super::status::StatusRegistry,
}
impl Selection {
    pub fn new(config: &NativeDiscordConfig) -> Self {
        Self {
            enabled: config.enabled,
            discovery: config.enabled && config.discovery_enabled,
            records: Arc::new(RwLock::new(if config.enabled {
                config.records.clone()
            } else {
                BTreeSet::new()
            })),
            status: Default::default(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn discovery_enabled(&self) -> bool {
        self.discovery
    }
    pub fn contains(&self, record: &str) -> bool {
        self.records.read().unwrap().contains(record)
    }
    pub fn records(&self) -> BTreeSet<String> {
        self.records.read().unwrap().clone()
    }
    pub fn len(&self) -> usize {
        self.records.read().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn status(&self) -> super::status::StatusRegistry {
        self.status.clone()
    }
    pub(super) fn insert(&self, record: String) -> bool {
        let mut records = self.records.write().unwrap();
        if records.contains(&record) {
            return true;
        }
        records.len() < MAX_RECORDS && records.insert(record)
    }
    pub(super) fn remove(&self, record: &str) {
        self.records.write().unwrap().remove(record);
        self.status.remove(record);
    }
}

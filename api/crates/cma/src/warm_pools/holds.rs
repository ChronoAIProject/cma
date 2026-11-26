//! Protect the warm-pool lane whose configured runtime pin was refused.
use super::NativeWarmPools;
use std::sync::Arc;

/// Images whose fixed-pin startup failed the durable pool activation fence.
/// Startup records a hold before opening admission or starting any workers.
#[derive(Default)]
pub(crate) struct DefaultPoolHolds(std::sync::RwLock<[Option<String>; 2]>);

impl DefaultPoolHolds {
    pub(super) fn hold(&self, machine: bool, image: String) {
        self.0.write().expect("default pool holds")[usize::from(machine)] = Some(image);
    }

    pub(crate) fn lane(&self, machine: bool) -> bool {
        self.0.read().expect("default pool holds")[usize::from(machine)].is_some()
    }

    pub(crate) fn image(&self, image: &str) -> bool {
        self.0
            .read()
            .expect("default pool holds")
            .iter()
            .flatten()
            .any(|held| held == image)
    }
}

impl NativeWarmPools {
    pub(crate) fn with_default_pool_holds(
        mut self: Arc<Self>,
        holds: Arc<DefaultPoolHolds>,
    ) -> Arc<Self> {
        Arc::make_mut(&mut self).default_holds = holds;
        self
    }
}

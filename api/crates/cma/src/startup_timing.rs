//! Bounded, payload-free observations of startup barriers.
use std::{future::Future, time::Instant};

use crate::config::ManagedMachineUpdates;

pub(crate) fn observe_managed_machine_scope(
    scope: ManagedMachineUpdates,
    runtime_image_origin_enabled: bool,
) {
    if runtime_image_origin_enabled && scope == ManagedMachineUpdates::Disabled {
        tracing::warn!(
            target: "cma_startup",
            parent: None,
            setting = "CHRONO_MANAGED_MACHINE_RUNTIME_UPDATES",
            mode = scope.mode(),
            runtime_image_origin_enabled,
            "Disabled Machine scope blocks origin qualification and new managed Machine update admission"
        );
    } else {
        tracing::info!(
            target: "cma_startup",
            parent: None,
            setting = "CHRONO_MANAGED_MACHINE_RUNTIME_UPDATES",
            mode = scope.mode(),
            runtime_image_origin_enabled,
            "Managed Machine runtime update scope"
        );
    }
}

pub(crate) struct Phase {
    name: &'static str,
    started: Instant,
    finished: bool,
}

impl Phase {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            started: Instant::now(),
            finished: false,
        }
    }

    pub(crate) fn finish(mut self, outcome: &'static str, count: Option<usize>) {
        self.finished = true;
        self.emit(outcome, count);
    }

    fn emit(&self, outcome: &'static str, count: Option<usize>) {
        tracing::info!(
            target: "cma_startup",
            parent: None,
            phase = self.name,
            elapsed_ms = self.started.elapsed().as_millis(),
            outcome,
            item_count = count.unwrap_or(0),
            count_known = count.is_some(),
            "API startup phase completed"
        );
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if !self.finished {
            self.emit("cancelled", None);
        }
    }
}

pub(crate) async fn observe<T, E>(
    name: &'static str,
    future: impl Future<Output = Result<T, E>>,
    count: impl FnOnce(&T) -> Option<usize>,
) -> Result<T, E> {
    let phase = Phase::new(name);
    let result = future.await;
    phase.finish(
        if result.is_ok() { "ok" } else { "error" },
        result.as_ref().ok().and_then(count),
    );
    result
}

#[cfg(test)]
#[path = "startup_timing_tests.rs"]
mod tests;

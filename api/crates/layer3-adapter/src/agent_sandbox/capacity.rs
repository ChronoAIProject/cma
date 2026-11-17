//! One process-local authority; ordinary admission cannot spend continuity reserve.
mod model;
mod queue;
#[cfg(test)]
mod tests;

pub use model::RuntimePermitKind;
pub use model::{
    RuntimeCapacityConfig, RuntimeCapacityFailure, RuntimeCapacityReason, RuntimeWorkloadClass,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Inject the same authority into every native client in one serving process.
#[derive(Debug)]
pub struct RuntimeCapacityAuthority {
    config: RuntimeCapacityConfig,
    pub(crate) streams: Arc<Semaphore>,
    pub(crate) requests: Arc<Semaphore>,
    state: Mutex<State>,
    wake: Notify,
}

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<u64>,
    next_ticket: u64,
}

/// Ownership covers exactly one stream lifetime or one request operation.
#[derive(Debug)]
pub(crate) struct RuntimePermit {
    permit: Option<OwnedSemaphorePermit>,
    authority: Arc<RuntimeCapacityAuthority>,
    kind: RuntimePermitKind,
}

impl RuntimePermit {
    pub(crate) fn proves_request(&self, authority: &Arc<RuntimeCapacityAuthority>) -> bool {
        self.kind == RuntimePermitKind::Request && Arc::ptr_eq(&self.authority, authority)
    }
}

impl Drop for RuntimePermit {
    fn drop(&mut self) {
        let _state = self
            .authority
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(self.permit.take());
        self.authority.record_occupancy(self.kind);
        self.authority.wake.notify_waiters();
    }
}

impl RuntimeCapacityAuthority {
    pub fn new(config: RuntimeCapacityConfig) -> Result<Arc<Self>, String> {
        let config = config.validate()?;
        let authority = Arc::new(Self {
            streams: Arc::new(Semaphore::new(config.stream_limit)),
            requests: Arc::new(Semaphore::new(config.request_limit)),
            config,
            state: Mutex::new(State::default()),
            wake: Notify::new(),
        });
        for kind in [RuntimePermitKind::Stream, RuntimePermitKind::Request] {
            let (limit, reserve) = authority.limits(kind);
            for (scope, value) in [
                ("physical", limit),
                ("reserve", reserve),
                ("usable", limit - reserve),
            ] {
                metrics::gauge!("cma_adapter_runtime_capacity", "kind" => kind.label(), "scope" => scope).set(value as f64);
            }
            authority.record_occupancy(kind);
        }
        authority.record_queue(0);
        Ok(authority)
    }

    pub fn config(&self) -> RuntimeCapacityConfig {
        self.config
    }

    pub fn available_streams(&self) -> usize {
        self.streams.available_permits()
    }

    pub fn available_requests(&self) -> usize {
        self.requests.available_permits()
    }

    fn limits(&self, kind: RuntimePermitKind) -> (usize, usize) {
        match kind {
            RuntimePermitKind::Stream => (self.config.stream_limit, self.config.stream_reserve),
            RuntimePermitKind::Request => (self.config.request_limit, self.config.request_reserve),
        }
    }

    fn semaphore(&self, kind: RuntimePermitKind) -> &Arc<Semaphore> {
        match kind {
            RuntimePermitKind::Stream => &self.streams,
            RuntimePermitKind::Request => &self.requests,
        }
    }

    // Caller holds state: checking reserve and physical acquisition is atomic.
    fn take(
        self: &Arc<Self>,
        kind: RuntimePermitKind,
        class: RuntimeWorkloadClass,
    ) -> Option<RuntimePermit> {
        let semaphore = self.semaphore(kind);
        if class == RuntimeWorkloadClass::Ordinary
            && semaphore.available_permits() <= self.limits(kind).1
        {
            return None;
        }
        let permit = semaphore.clone().try_acquire_owned().ok()?;
        self.record_occupancy(kind);
        metrics::counter!("cma_adapter_runtime_acquisitions_total", "kind" => kind.label(), "class" => class.label(), "outcome" => "acquired").increment(1);
        Some(RuntimePermit {
            permit: Some(permit),
            authority: self.clone(),
            kind,
        })
    }

    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        kind: RuntimePermitKind,
        class: RuntimeWorkloadClass,
    ) -> Result<RuntimePermit, RuntimeCapacityFailure> {
        let class = class.effective();
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let may_take = kind != RuntimePermitKind::Request
            || class != RuntimeWorkloadClass::Ordinary
            || state.queue.is_empty();
        if may_take {
            if let Some(permit) = self.take(kind, class) {
                return Ok(permit);
            }
        }
        Err(self.refuse(kind, class, RuntimeCapacityReason::Exhausted))
    }

    fn refuse(
        &self,
        kind: RuntimePermitKind,
        class: RuntimeWorkloadClass,
        reason: RuntimeCapacityReason,
    ) -> RuntimeCapacityFailure {
        let failure = RuntimeCapacityFailure {
            kind,
            class,
            reason,
        };
        metrics::counter!("cma_adapter_runtime_refusals_total", "kind" => kind.label(), "class" => class.label(), "reason" => reason.label()).increment(1);
        failure
    }

    fn record_occupancy(&self, kind: RuntimePermitKind) {
        let available = self.semaphore(kind).available_permits();
        metrics::gauge!("cma_adapter_runtime_permits_in_use", "kind" => kind.label())
            .set((self.limits(kind).0 - available) as f64);
        metrics::gauge!("cma_adapter_runtime_permits_available", "kind" => kind.label())
            .set(available as f64);
    }

    fn record_queue(&self, depth: usize) {
        metrics::gauge!("cma_adapter_runtime_queue_depth", "kind" => "request").set(depth as f64);
    }
}

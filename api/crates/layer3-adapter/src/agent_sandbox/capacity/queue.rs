use super::*;
use tokio::time::Instant;

// RAII removes cancelled waiters synchronously; no detached cleanup task and no
// abandoned ticket can strand later work. Only ordinary requests queue.
struct Ticket {
    authority: Arc<RuntimeCapacityAuthority>,
    id: u64,
    started: Instant,
    outcome: &'static str,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self
            .authority
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.queue.retain(|id| *id != self.id);
        self.authority.record_queue(state.queue.len());
        metrics::histogram!("cma_adapter_runtime_queue_wait_seconds", "kind" => "request", "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
        self.authority.wake.notify_waiters();
    }
}
impl RuntimeCapacityAuthority {
    pub(crate) async fn acquire_request(
        self: &Arc<Self>,
        class: RuntimeWorkloadClass,
    ) -> Result<RuntimePermit, RuntimeCapacityFailure> {
        let class = class.effective();
        if class != RuntimeWorkloadClass::Ordinary || self.config.request_queue_limit == 0 {
            return self.try_acquire(RuntimePermitKind::Request, class);
        }
        let mut ticket = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.queue.is_empty() {
                if let Some(permit) = self.take(RuntimePermitKind::Request, class) {
                    return Ok(permit);
                }
            }
            if state.queue.len() >= self.config.request_queue_limit {
                return Err(self.refuse(
                    RuntimePermitKind::Request,
                    class,
                    RuntimeCapacityReason::QueueFull,
                ));
            }
            state.next_ticket = state.next_ticket.wrapping_add(1);
            let id = state.next_ticket;
            state.queue.push_back(id);
            metrics::counter!("cma_adapter_runtime_queued_total", "kind" => "request").increment(1);
            self.record_queue(state.queue.len());
            Ticket {
                authority: self.clone(),
                id,
                started: Instant::now(),
                outcome: "cancelled",
            }
        };
        let deadline = ticket.started + self.config.request_queue_max_wait;
        loop {
            if Instant::now() >= deadline {
                ticket.outcome = "timed_out";
                return Err(self.refuse(
                    RuntimePermitKind::Request,
                    class,
                    RuntimeCapacityReason::QueueTimedOut,
                ));
            }
            // Register before checking availability so release cannot be missed.
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.queue.front() == Some(&ticket.id) {
                    if let Some(permit) = self.take(RuntimePermitKind::Request, class) {
                        state.queue.pop_front();
                        ticket.outcome = "acquired";
                        return Ok(permit);
                    }
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                ticket.outcome = "timed_out";
                return Err(self.refuse(
                    RuntimePermitKind::Request,
                    class,
                    RuntimeCapacityReason::QueueTimedOut,
                ));
            }
        }
    }
}

use std::time::Duration;

pub use crate::RuntimeCapacityKind as RuntimePermitKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeWorkloadClass {
    Ordinary,
    Continuity,
    Control,
}
tokio::task_local! { static WORKLOAD: RuntimeWorkloadClass; }

impl RuntimeWorkloadClass {
    /// Carry already-proved admitted-work context through nested runtime reads.
    /// This is in-memory admission policy, never a serialized request field.
    pub async fn scope<F: std::future::Future>(self, future: F) -> F::Output {
        WORKLOAD.scope(self.effective(), future).await
    }
    pub(crate) fn effective(self) -> Self {
        if self == Self::Ordinary {
            WORKLOAD.try_with(|class| *class).unwrap_or(self)
        } else {
            self
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Continuity => "continuity",
            Self::Control => "control",
        }
    }
}

impl RuntimePermitKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Request => "request",
        }
    }
}

/// Deployment policy. Validation maxima are not qualified operating values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeCapacityConfig {
    pub stream_limit: usize,
    pub request_limit: usize,
    pub stream_reserve: usize,
    pub request_reserve: usize,
    pub request_queue_limit: usize,
    pub request_queue_max_wait: Duration,
}
impl Default for RuntimeCapacityConfig {
    fn default() -> Self {
        Self {
            stream_limit: 64,
            request_limit: 32,
            stream_reserve: 0,
            request_reserve: 0,
            request_queue_limit: 0,
            request_queue_max_wait: Duration::from_secs(1),
        }
    }
}
impl RuntimeCapacityConfig {
    pub fn validate(self) -> Result<Self, String> {
        for (name, limit, reserve) in [
            ("stream", self.stream_limit, self.stream_reserve),
            ("request", self.request_limit, self.request_reserve),
        ] {
            if !(1..=4096).contains(&limit) {
                return Err(format!("runtime {name} limit must be 1..4096"));
            }
            if reserve >= limit {
                return Err(format!(
                    "runtime {name} reserve must be less than its limit"
                ));
            }
        }
        if self.request_queue_limit > 1024 {
            return Err("runtime request queue limit must be 0..1024".into());
        }
        if !(Duration::from_millis(1)..=Duration::from_secs(60))
            .contains(&self.request_queue_max_wait)
        {
            return Err("runtime request queue wait must be 1..60000 milliseconds".into());
        }
        Ok(self)
    }
    pub fn usable_streams(self) -> usize {
        self.stream_limit.saturating_sub(self.stream_reserve)
    }
    pub fn usable_requests(self) -> usize {
        self.request_limit.saturating_sub(self.request_reserve)
    }
    /// Uniform placement assumption; this does not discover or admit a fleet.
    pub fn required_ready_instances(
        self,
        active: usize,
        concurrent_requests: usize,
    ) -> Result<usize, String> {
        self.validate()?;
        active
            .div_ceil(self.usable_streams())
            .max(concurrent_requests.div_ceil(self.usable_requests()))
            .checked_add(1)
            .ok_or_else(|| "runtime capacity arithmetic overflow".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCapacityReason {
    Exhausted,
    QueueFull,
    QueueTimedOut,
    NativeHttpSessions,
}
impl RuntimeCapacityReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exhausted => "exhausted",
            Self::QueueFull => "queue_full",
            Self::QueueTimedOut => "queue_timed_out",
            Self::NativeHttpSessions => "native_http_sessions",
        }
    }
}

/// Transient local capacity, never evidence that a runtime or receipt was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {reason}", code = self.code(), reason = .reason.label())]
pub struct RuntimeCapacityFailure {
    pub kind: RuntimePermitKind,
    pub class: RuntimeWorkloadClass,
    pub reason: RuntimeCapacityReason,
}
impl RuntimeCapacityFailure {
    pub fn code(self) -> &'static str {
        match self.kind {
            RuntimePermitKind::Stream => "runtime_stream_capacity_exhausted",
            RuntimePermitKind::Request => "runtime_request_capacity_exhausted",
        }
    }
    pub(crate) fn native_http_sessions(class: RuntimeWorkloadClass) -> Self {
        metrics::counter!("cma_adapter_runtime_refusals_total", "kind" => "request", "class" => class.label(), "reason" => "native_http_sessions").increment(1);
        Self {
            kind: RuntimePermitKind::Request,
            class,
            reason: RuntimeCapacityReason::NativeHttpSessions,
        }
    }
}

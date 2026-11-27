//! Request-scoped file-read observations, independent of logging and authority.
use std::{future::Future, sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Authorization,
    RuntimeStatus,
    HomeBeforeStatus,
    HomeAfterStatus,
    RuntimeRead,
    ReadBinding,
    ReadPolicy,
    ReceiptValidation,
    StatusEndpoint,
    ReadEndpoint,
    StatusControl,
    ReadControl,
    StatusPostValidation,
    FinalFence,
}
impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authorization => "auth",
            Self::RuntimeStatus => "runtime_status",
            Self::HomeBeforeStatus => "home_before_status",
            Self::HomeAfterStatus => "home_after_status",
            Self::RuntimeRead => "runtime_read",
            Self::ReadBinding => "read_binding",
            Self::ReadPolicy => "read_policy",
            Self::ReceiptValidation => "receipt_validation",
            Self::StatusEndpoint => "status_endpoint",
            Self::ReadEndpoint => "read_endpoint",
            Self::StatusControl => "status_control",
            Self::ReadControl => "read_control",
            Self::StatusPostValidation => "status_post_validation",
            Self::FinalFence => "final_fence",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Error,
    Cancelled,
}
impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy)]
pub enum TransportPhase {
    Endpoint,
    Control,
}

/// Only these two exchanges belong to the file-read protocol. Other operations
/// in a shared transport retain their existing behavior without a false label.
pub async fn transport<T, E>(
    operation: crate::wire::WireOperation,
    phase: TransportPhase,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    use crate::wire::WireOperation;
    let stage = match (operation, phase) {
        (WireOperation::RuntimeStatus, TransportPhase::Endpoint) => Stage::StatusEndpoint,
        (WireOperation::ReadAgentFile, TransportPhase::Endpoint) => Stage::ReadEndpoint,
        (WireOperation::RuntimeStatus, TransportPhase::Control) => Stage::StatusControl,
        (WireOperation::ReadAgentFile, TransportPhase::Control) => Stage::ReadControl,
        _ => return future.await,
    };
    measure(stage, future).await
}

type Observer = Arc<dyn Fn(Stage, Outcome, u64) + Send + Sync>;
tokio::task_local! { static OBSERVER: Observer; }

/// The caller chooses whether to opt in. This scope never changes the result,
/// caches authority, spawns work, or extends the lifetime of a read.
pub async fn scope<T>(
    observer: impl Fn(Stage, Outcome, u64) + Send + Sync + 'static,
    operation: impl Future<Output = T>,
) -> T {
    OBSERVER.scope(Arc::new(observer), operation).await
}

struct Timing {
    observer: Observer,
    stage: Stage,
    started: Instant,
    outcome: Outcome,
}
impl Drop for Timing {
    fn drop(&mut self) {
        (self.observer)(
            self.stage,
            self.outcome,
            u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
    }
}

pub async fn measure<T, E>(
    stage: Stage,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut timing = OBSERVER
        .try_with(|observer| Timing {
            observer: observer.clone(),
            stage,
            started: Instant::now(),
            outcome: Outcome::Cancelled,
        })
        .ok();
    let result = operation.await;
    if let Some(timing) = timing.as_mut() {
        timing.outcome = if result.is_ok() {
            Outcome::Completed
        } else {
            Outcome::Error
        };
    }
    result
}

#[cfg(test)]
mod tests;

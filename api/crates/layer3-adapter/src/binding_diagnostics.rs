//! Fixed, identity-free diagnostics scoped to one bounded binding attempt.
//! These observations never carry proof, retry effects, or detach pending I/O.
use std::future::Future;
use tokio::time::Instant;

tokio::task_local! { static DEADLINE: Instant; }

#[derive(Clone, Copy, Debug)]
pub enum Stage {
    DurableRead,
    Adoption,
    NativeIdentity,
    NativeReadiness,
    NativeAssignment,
    TransportWait,
    Status,
    FinalLossObservation,
    InterruptionObservation,
    Transaction,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::DurableRead => "durable_read",
            Self::Adoption => "adoption",
            Self::NativeIdentity => "native_identity",
            Self::NativeReadiness => "native_readiness",
            Self::NativeAssignment => "native_assignment",
            Self::TransportWait => "transport_wait",
            Self::Status => "status",
            Self::FinalLossObservation => "final_loss_observation",
            Self::InterruptionObservation => "interruption_observation",
            Self::Transaction => "transaction",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Complete,
    Absent,
    Refused,
    Unavailable,
    Capacity,
    Conflict,
    Corrupt,
    Superseded,
    NotEligible,
    RecoveryScheduled,
    Failed,
    Deadline,
    Cancelled,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Absent => "absent",
            Self::Refused => "refused",
            Self::Unavailable => "unavailable",
            Self::Capacity => "capacity",
            Self::Conflict => "conflict",
            Self::Corrupt => "corrupt",
            Self::Superseded => "superseded",
            Self::NotEligible => "not_eligible",
            Self::RecoveryScheduled => "recovery_scheduled",
            Self::Failed => "failed",
            Self::Deadline => "deadline",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The deadline is diagnostic context only; the caller owns cancellation.
pub async fn scope<T>(deadline: Instant, operation: impl Future<Output = T>) -> T {
    DEADLINE.scope(deadline, operation).await
}

struct Timing {
    stage: Stage,
    started: Instant,
    deadline: Instant,
    outcome: Option<Outcome>,
}

impl Drop for Timing {
    fn drop(&mut self) {
        let outcome = self.outcome.unwrap_or_else(|| {
            if Instant::now() >= self.deadline {
                Outcome::Deadline
            } else {
                Outcome::Cancelled
            }
        });
        tracing::info!(parent: None,
            stage = self.stage.name(), outcome = outcome.name(),
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "sandbox binding stage completed"
        );
    }
}

/// Preserve the original result, including errors and uncertain effects.
pub async fn measure<T>(
    stage: Stage,
    operation: impl Future<Output = T>,
    classify: impl FnOnce(&T) -> Outcome,
) -> T {
    let mut timing = DEADLINE
        .try_with(|deadline| Timing {
            stage,
            started: Instant::now(),
            deadline: *deadline,
            outcome: None,
        })
        .ok();
    let result = operation.await;
    if let Some(timing) = timing.as_mut() {
        timing.outcome = Some(classify(&result));
    }
    result
}

pub fn adapter_outcome<T>(result: &crate::Result<T>) -> Outcome {
    use crate::AdapterError;
    match result {
        Ok(_) => Outcome::Complete,
        Err(AdapterError::Unavailable(_) | AdapterError::InputAuthorityUnavailable(_)) => {
            Outcome::Unavailable
        }
        Err(AdapterError::AtCapacity(_)) => Outcome::Capacity,
        Err(AdapterError::OperationIdentityConflict(_)) => Outcome::Conflict,
        Err(_) => Outcome::Refused,
    }
}

pub(crate) async fn adapter<T>(
    stage: Stage,
    operation: impl Future<Output = crate::Result<T>>,
) -> crate::Result<T> {
    measure(stage, operation, adapter_outcome).await
}

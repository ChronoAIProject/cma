//! Fenced execution of committed routed targets; no ingress activation.
//!
//! A local lease cannot fence a network request already sent. A restart retries
//! an uncertain start only when its provider proves exact-body idempotency;
//! otherwise it retains Unknown. Platform delivery remains a separate effect.
mod body;
mod dispatcher;
mod journal;
mod model;
mod storage;

pub use dispatcher::{Authority, DispatchOutcome, Dispatcher, ExecutionPort, StartResult};
pub use journal::Journal;
pub use model::{
    ExecutionError, FrozenDelivery, FrozenTurn, Lease, Phase, RecoverySupport, TargetState,
};

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

//! Fenced execution of committed routed targets; no ingress activation.
//!
//! A local lease cannot fence a network request already sent. A restart retries
//! an uncertain start only when its provider proves exact-body idempotency;
//! otherwise it retains Unknown. Platform delivery remains a separate effect.
mod adapter;
mod body;
mod builder;
mod delivery;
mod delivery_gate;
mod delivery_state;
mod dispatcher;
mod journal;
mod model;
pub mod scheduler;
mod storage;

pub use adapter::{FrozenTurnBuilder, HttpExecutionPort, ReattachError, ReattachedResponse};
pub use builder::ProductionFrozenTurnBuilder;
pub use delivery::{FrameDelivery, ReplayEffect, RoutedDelivery};
pub use delivery_state::{DeliveryCheckpoint, DeliveryStatus};
pub use dispatcher::{Authority, DispatchOutcome, Dispatcher, ExecutionPort, StartResult};
pub use journal::Journal;
pub use model::{
    ExecutionError, FrozenDelivery, FrozenTurn, Lease, Phase, RecoverySupport, TargetState,
};

#[cfg(test)]
mod adapter_test_server;
#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod delivery_crash_tests;
#[cfg(test)]
mod delivery_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod scheduler_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod builder_test_media;
#[cfg(test)]
mod builder_tests;

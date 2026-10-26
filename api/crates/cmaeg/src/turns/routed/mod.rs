//! Fenced execution of committed routed targets; no ingress activation.
//!
//! A local lease cannot fence a network request already sent. A restart retries
//! an uncertain start only when its provider proves exact-body idempotency;
//! otherwise it retains Unknown. Platform delivery remains a separate effect.
mod adapter;
mod body;
mod builder;
mod custom_delivery;
mod custom_state;
mod delivery;
mod delivery_gate;
mod delivery_state;
mod destinations;
mod dispatcher;
mod executor;
mod journal;
mod model;
mod projection_store;
mod replay_projection;
pub mod scheduler;
mod storage;
mod tool_operation;
mod tool_replies;
mod tool_reply_gate;
mod tool_reply_index;
mod tool_reply_store;

pub use adapter::{FrozenTurnBuilder, HttpExecutionPort, ReattachError, ReattachedResponse};
pub use builder::ProductionFrozenTurnBuilder;
pub use custom_state::{CustomCheckpoint, WebhookLeg, WebhookStatus};
pub use delivery::{FrameDelivery, ReplayEffect, RoutedDelivery};
pub use delivery_state::{DeliveryCheckpoint, DeliveryStatus};
pub use destinations::{
    validate_routed_delivery, validate_routed_delivery_with_sync, DefaultDestination,
};
pub use dispatcher::{Authority, DispatchOutcome, Dispatcher, ExecutionPort, StartResult};
pub use executor::RoutedTargetExecutor;
pub use journal::Journal;
pub use model::{
    ExecutionError, FrozenDelivery, FrozenTurn, Lease, Phase, RecoverySupport, TargetState,
};
pub use tool_replies::RoutedToolReplies;
pub use tool_reply_store::ToolReplyStatus;

#[cfg(test)]
mod adapter_test_server;
#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod custom_delivery_tests;
#[cfg(test)]
mod custom_http_fixture;
#[cfg(test)]
mod custom_http_tests;
#[cfg(test)]
mod delivery_crash_tests;
#[cfg(test)]
mod delivery_tests;
#[cfg(test)]
mod destination_tests;
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

#[cfg(test)]
mod executor_tests;
#[cfg(test)]
mod projection_store_tests;
#[cfg(test)]
mod tool_reply_tests;

#[cfg(test)]
mod executor_tool_tests;

#[cfg(test)]
mod tool_operation_tests;

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
mod delivery_receipts;
mod delivery_state;
mod destinations;
mod dispatcher;
mod dispatcher_authority;
mod effect_roots;
mod event_attestation;
mod execution_port;
mod executor;
mod expiry;
mod generation_provenance;
mod journal;
mod live_scope;
mod measured_executor;
mod media_delivery;
mod media_delivery_gate;
#[cfg(test)]
mod media_delivery_tests;
mod media_obligations;
mod media_response_store;
mod media_schedule;
mod media_status;
mod media_work;
mod media_worker;
mod model;
mod non_admission;
pub(crate) mod operator;
#[cfg(test)]
mod operator_tests;
mod partition_fence;
mod preparation;
#[cfg(test)]
mod provider_start_tests;
pub(crate) mod quarantine;
pub use preparation::{BindingPreparation, PreparationIntent, PreparationReceipt};
#[cfg(test)]
mod custom_preparation_fixture;
#[cfg(test)]
mod custom_preparation_tests;
#[cfg(test)]
mod preparation_dispatch_tests;
#[cfg(test)]
mod preparation_tests;
mod projection_store;
mod received_message;
mod received_source;
pub(crate) use received_source::retained_received_source;
mod replay_frame;
mod replay_projection;
pub mod scheduler;
mod side_message_authority;
pub(crate) use side_message_authority::SideMessageAuthority;
mod storage;
mod stream_relay;
pub(crate) use stream_relay::monitor::StreamMonitor;
mod tool_effects;
mod tool_operation;
mod tool_relay;
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
pub use event_attestation::{EventAttestationRequest, EventAttestationResponse};
pub use executor::RoutedTargetExecutor;
pub use journal::Journal;
pub(crate) use media_delivery::MediaDelivery;
pub use media_obligations::{
    MediaContinuationReceipt, MediaLease, MediaObligationStore, MediaUploadReceipt,
};
pub use media_status::{MediaResponsePage, RetainedMediaResponse};
pub(crate) use media_work::MediaWorkIndex;
pub(crate) use media_worker::MediaWorker;
pub use model::{
    ExecutionError, FrozenDelivery, FrozenTurn, Lease, Phase, RecoverySupport, TargetState,
};
pub use received_message::current_received_message;
#[cfg(test)]
pub(crate) use received_message::{
    bind_received_message_fixture, bind_received_message_fixture_with_source,
};
pub use tool_replies::RoutedToolReplies;
pub use tool_reply_store::ToolReplyStatus;

#[cfg(test)]
pub(crate) mod adapter_test_server;
#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod custom_delivery_tests;
#[cfg(test)]
mod custom_http_fixture;
#[cfg(test)]
mod custom_http_media_tests;
#[cfg(test)]
mod custom_http_retry_tests;
#[cfg(test)]
mod custom_http_tests;
#[cfg(test)]
mod custom_http_webhook_tests;
#[cfg(test)]
mod delivery_crash_tests;
#[cfg(test)]
pub(crate) mod delivery_tests;
#[cfg(test)]
mod destination_tests;
#[cfg(test)]
mod long_queue_tests;
#[cfg(test)]
mod readiness_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod scheduler_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod builder_test_media;
#[cfg(test)]
mod builder_tests;

#[cfg(test)]
mod executor_activity_tests;
#[cfg(test)]
pub(crate) mod executor_tests;
#[cfg(test)]
mod expiry_tests;
#[cfg(test)]
mod oversized_frame_tests;
#[cfg(test)]
mod preparation_activity_tests;
#[cfg(test)]
mod projection_store_tests;
#[cfg(test)]
mod replay_trace_tests;
#[cfg(test)]
mod tool_reply_tests;

#[cfg(test)]
mod executor_tool_tests;

#[cfg(test)]
mod tool_operation_tests;

#[cfg(test)]
mod telegram_http_retry_tests;
#[cfg(test)]
mod whatsapp_execution_tests;

#[cfg(test)]
mod native_lark_execution_tests;
#[cfg(test)]
mod whatsapp_group_tests;
#[cfg(test)]
mod whatsapp_template_tests;

#[cfg(test)]
mod native_lark_outbound_tests;
#[cfg(test)]
mod native_lark_thread_start_tests;

#[cfg(test)]
mod native_lark_sticker_tests;

#[cfg(test)]
mod telegram_structured_tests;

#[cfg(test)]
mod whatsapp_structured_tests;

#[cfg(test)]
mod replica_test_fixture;
#[cfg(test)]
mod replica_tool_tests;

mod media_effects;

#[cfg(test)]
mod replica_legacy_tool_tests;

#[cfg(test)]
mod compatibility_execution_tests;

#[cfg(test)]
mod compatibility_mongo_tests;
mod execution_activity;

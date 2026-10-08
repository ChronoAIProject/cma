//! Channel-definition construction and management orchestration.

pub mod audit;
mod audit_pages;
pub mod audit_routing;
pub mod audit_routing_clear;
pub mod cache;
mod consumer_binding;
mod consumer_build;
pub mod consumer_management;
pub mod consumer_requests;
mod consumers;
pub mod creation;
pub mod definition;
mod definition_sources;
#[cfg(test)]
mod definition_write_tests;
mod lifecycle;
pub mod management;
mod mutation_metrics;
pub mod observer;
mod requests;
pub mod resolver;
/// The routed channel the core's routing tests use, shared by this crate's tests.
#[cfg(test)]
#[path = "../../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub(crate) mod routing_fixture;
pub mod routing_policy;
mod source_secrets;
pub mod sources;
pub(crate) mod state;
pub mod target;
mod test_turn;
pub mod warnings;
pub mod write_gate;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};
pub use target::ChannelTarget;

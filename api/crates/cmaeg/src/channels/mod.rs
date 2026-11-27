//! Channel-definition construction and management orchestration.

pub mod audit;
mod audit_pages;
pub mod audit_routing;
pub mod audit_routing_clear;
pub mod cache;
mod consumers;
pub mod creation;
mod custom_execution;
pub mod definition;
mod definition_sources;
mod github_sources;
pub(crate) use definition_sources::check_github_sources;
mod inspection;
mod lifecycle;
pub mod management;
pub(crate) mod mutation;
pub mod observer;
pub(crate) mod profile_assignment;
pub(crate) mod profile_revision;
mod relay_telegram;
mod requests;
pub mod resolver;
/// The routed channel the core's routing tests use, shared by this crate's tests.
#[cfg(test)]
#[path = "../../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub(crate) mod routing_fixture;
pub mod routing_update;
pub mod source_check;
pub(crate) mod state;
mod test_turn;
pub mod write_gate;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

mod native_discord;
mod native_telegram;

#[cfg(test)]
mod create_cleanup_tests;
#[cfg(test)]
mod lark_effect_gate_tests;

#[cfg(test)]
mod create_fence_tests;
#[cfg(test)]
mod mutation_audit_tests;
#[cfg(test)]
mod mutation_bounds_tests;
#[cfg(test)]
mod mutation_fixture;
#[cfg(test)]
mod mutation_tests;
#[cfg(test)]
mod setup_refusal_tests;

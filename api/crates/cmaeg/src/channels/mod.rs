//! Channel-definition construction and management orchestration.

pub mod audit;
mod audit_pages;
pub mod cache;
mod consumers;
pub mod creation;
pub mod definition;
mod definition_sources;
mod lifecycle;
pub mod management;
mod requests;
pub mod resolver;
/// The routed channel the core's routing tests use, shared by this crate's tests.
#[cfg(test)]
#[path = "../../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
pub(crate) mod state;
mod test_turn;
pub mod write_gate;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

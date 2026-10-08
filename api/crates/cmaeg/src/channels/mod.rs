//! Channel-definition construction and management orchestration.

pub mod audit;
pub mod cache;
mod consumers;
pub mod creation;
pub mod definition;
mod definition_sources;
mod lifecycle;
pub mod management;
mod requests;
pub mod resolver;
pub(crate) mod state;
mod test_turn;
pub mod write_gate;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

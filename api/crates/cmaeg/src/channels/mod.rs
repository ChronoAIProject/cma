//! Channel-definition construction and management orchestration.

pub mod audit;
pub mod cache;
mod consumers;
pub mod creation;
pub mod definition;
mod definition_sources;
mod lifecycle;
pub mod management;
pub mod observer;
mod requests;
pub mod resolver;
pub(crate) mod state;
mod test_turn;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

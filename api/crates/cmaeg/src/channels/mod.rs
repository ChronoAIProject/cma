//! Channel-definition construction and management orchestration.

pub mod cache;
pub mod creation;
pub mod definition;
pub mod management;
mod requests;
pub mod resolver;
pub(crate) mod state;
mod test_turn;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

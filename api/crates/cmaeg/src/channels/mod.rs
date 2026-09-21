//! Channel-definition construction and management orchestration.

pub mod cache;
pub mod definition;
pub mod management;
pub mod resolver;
pub(crate) mod state;
mod test_turn;

pub use definition::{
    ChannelCreateRequest, ChannelUpdateRequest, CredentialPatch, CredentialsRequest,
    HmacSecretRequest, ProviderRequest, SourceRequest,
};

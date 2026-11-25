//! Owner Bot lifecycle. External protocol orchestration implements the execution port.
mod authority;
pub use authority::{BotDelegation, BotOwnerAuthority, BotOwnership};
mod control;
mod creation;
pub use control::*;
mod effects;
mod execution;
mod model;
mod profiles;
mod resolver;
mod rotation;
mod validation;
pub use resolver::*;
mod service;
mod store;
pub use execution::*;
pub use model::*;
pub use profiles::*;
pub use service::*;
pub use store::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BotError {
    #[error("bot management is forbidden")]
    Forbidden,
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error("bot not found")]
    NotFound,
    #[error("bot operation conflicts with current work")]
    Conflict,
    #[error("{0}")]
    Invalid(String),
    #[error("new custom image selections are unavailable")]
    ImageSelectionUnsupported,
    #[error("the selected services need to be connected before creating this consumer")]
    ConnectionRequired {
        services: Vec<String>,
        connection_target: serde_json::Value,
    },
    #[error("bot dependency is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod execution_tests;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod ownership_tests;
